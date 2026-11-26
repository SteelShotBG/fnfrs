//! Core V-Slice gameplay session state (`PlayState.hx`) coordinating
//! `Conductor`, dual `Strumline`s, `InputQueue`, `PopUpPool`, `ScoreTallies`,
//! health mechanics, and `AudioEngine` SFX/stem controls with zero per-frame
//! heap allocations.

use std::time::Instant;

use fnfrs_audio::{AudioCommand, AudioEngine, TrackKind};
use fnfrs_core::conductor::{Conductor, ConductorEvents};
use fnfrs_core::constants::{
    COUNTDOWN_VOLUME, HEALTH_HOLD_BONUS_PER_SECOND, HEALTH_MAX, HEALTH_MIN, HEALTH_MISS_PENALTY,
    HEALTH_STARTING, MS_PER_SEC, SCORE_GHOST_MISS_PENALTY, SCORE_HOLD_BONUS_PER_SECOND,
    STRUMLINE_SIZE, STRUMLINE_X_OFFSET, STRUMLINE_Y_OFFSET, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::pool::PoolHandle;
use fnfrs_core::schema::song::{SongChartData, SongMetadata, SongNoteData};
use fnfrs_core::scoring::{
    format_score_into, get_miss_score, judge_note, score_note, Judgement, ScoreTallies,
    ScoringSystem,
};
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, MeshBatcher, SpriteBatcher, SpriteInstance, TextAlign,
    TextStyle, TextureHandle,
};

use crate::camera::{CameraRig, FlxEase, DEFAULT_STAGE_ZOOM};
use crate::character::BaseCharacter;
use crate::countdown::{CountdownAssets, CountdownController, CountdownSfx, CountdownStep};
use crate::events::{
    AnimationTarget, CameraFocusTarget, FocusCameraEase, GameEventAction, QueuedSongEvent,
    ScrollSpeedTarget, SongEventQueue, ZoomMode,
};
use crate::hud::{
    GameOverEvents, GameOverPhase, GameOverState, HealthBarLayout, HealthIcon, PauseMenuState,
    ResultsSummaryState,
};
use crate::input::{compute_note_diff_ms, InputQueue, NoteDirection};
use crate::note_style::NoteStyleAssets;
use crate::popup::PopUpPool;
use crate::scripts::{NativeScriptHost, NoteKindTag};
use crate::stage::Stage;
use crate::strumline::{Strumline, StrumlineVisualStyle};
use crate::sustain::compute_hold_drop_penalty;

/// Configuration for initializing a [`PlaySession`].
#[derive(Debug, Clone)]
pub struct PlayConfig {
    pub scroll_speed: f64,
    pub is_downscroll: bool,
    pub is_botplay: bool,
    pub ghost_tapping: bool,
    pub practice_mode: bool,
    pub scoring_system: ScoringSystem,
    pub player_vocals_gain: f32,
    pub opponent_vocals_gain: f32,
    pub start_timestamp_ms: f64,
    pub use_countdown_sfx_slots: bool,
}

impl Default for PlayConfig {
    fn default() -> Self {
        Self {
            scroll_speed: 1.0,
            is_downscroll: false,
            is_botplay: false,
            ghost_tapping: false,
            practice_mode: false,
            scoring_system: ScoringSystem::Pbot1,
            player_vocals_gain: 1.0,
            opponent_vocals_gain: 1.0,
            start_timestamp_ms: 0.0,
            use_countdown_sfx_slots: false,
        }
    }
}

/// Optional SFX bank slot indices mapped to `AudioCommand::PlaySfx`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GameplaySfxSlots {
    pub miss_notes: [Option<usize>; 3],
}

/// Per-frame gameplay events returned by [`PlaySession::update`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayStepEvents {
    pub conductor: ConductorEvents,
    pub last_judgement: Option<Judgement>,
    pub last_note_diff_ms: Option<f64>,
    pub missed_note: bool,
    pub ghost_missed: bool,
    pub missed_dir: Option<NoteDirection>,
    pub died: bool,
    pub player_hit_dir: Option<NoteDirection>,
    pub player_hit_kind: NoteKindTag,
    pub opponent_hit_dir: Option<NoteDirection>,
    pub opponent_hit_kind: NoteKindTag,
}

/// Headless-friendly, zero-allocation gameplay simulation matching `PlayState.hx`.
#[derive(Debug)]
pub struct PlaySession {
    pub config: PlayConfig,
    pub conductor: Conductor,
    pub player_strumline: Strumline,
    pub opponent_strumline: Strumline,
    pub popups: PopUpPool,
    pub tallies: ScoreTallies,

    /// Floating score accumulator (so per-second sustain hold points accumulate smoothly).
    pub song_score_f64: f64,
    pub health: f32,
    pub health_lerp: f32,
    pub is_paused: bool,
    pub is_player_dying: bool,
    pub sfx_slots: GameplaySfxSlots,
    pub hide_opponent_strumline: bool,
    pub hide_popups: bool,

    score_text_buf: [u8; 32],
    score_text_len: usize,
    strum_hitbox_height: f32,
    strum_offset_y: f32,
    rng_seed: u32,
}

impl PlaySession {
    /// Initializes a [`PlaySession`] from chart notes & time changes, pre-allocating all
    /// object pools upfront.
    #[must_use]
    pub fn new(
        metadata: &SongMetadata,
        chart: &SongChartData,
        difficulty: &str,
        mut config: PlayConfig,
    ) -> Self {
        if config.scroll_speed <= 0.0 {
            config.scroll_speed = f64::from(chart.get_scroll_speed(difficulty)).max(0.1);
        }

        let mut conductor = Conductor::new();
        conductor.map_time_changes(&metadata.time_changes);

        let strum_y = if config.is_downscroll {
            VIRTUAL_HEIGHT - STRUMLINE_SIZE - STRUMLINE_Y_OFFSET
        } else {
            STRUMLINE_Y_OFFSET
        };

        let player_x = VIRTUAL_WIDTH * 0.5 + STRUMLINE_X_OFFSET;
        let opponent_x = STRUMLINE_X_OFFSET;

        let mut player_strumline = Strumline::new(
            true,
            config.is_downscroll,
            config.scroll_speed,
            player_x,
            strum_y,
        );
        let mut opponent_strumline = Strumline::new(
            false,
            config.is_downscroll,
            config.scroll_speed,
            opponent_x,
            strum_y,
        );

        let chart_notes = chart.get_notes(difficulty);
        let mut player_notes = Vec::with_capacity(chart_notes.len());
        let mut opponent_notes = Vec::with_capacity(chart_notes.len());

        for n in chart_notes {
            if n.is_player_note(4) {
                player_notes.push(n.clone());
            } else {
                opponent_notes.push(n.clone());
            }
        }

        let total_notes = player_notes.len() as u32;
        player_strumline.apply_note_data(&player_notes);
        opponent_strumline.apply_note_data(&opponent_notes);

        let tallies = ScoreTallies {
            total_notes,
            ..Default::default()
        };

        let mut session = Self {
            config,
            conductor,
            player_strumline,
            opponent_strumline,
            popups: PopUpPool::default(),
            tallies,
            song_score_f64: 0.0,
            health: HEALTH_STARTING,
            health_lerp: HEALTH_STARTING,
            is_paused: false,
            is_player_dying: false,
            sfx_slots: GameplaySfxSlots::default(),
            hide_opponent_strumline: false,
            hide_popups: false,
            score_text_buf: [0u8; 32],
            score_text_len: 0,
            strum_hitbox_height: STRUMLINE_SIZE,
            strum_offset_y: 0.0,
            rng_seed: 0x1337_BEEF,
        };
        session.refresh_hud_buffers();
        session
    }

    /// Creates a session directly from arbitrary pre-split player & opponent note slices
    /// (convenient for isolated smoke demos and tests).
    #[must_use]
    pub fn from_raw_notes(
        bpm: f64,
        player_notes: &[SongNoteData],
        opponent_notes: &[SongNoteData],
        config: PlayConfig,
    ) -> Self {
        let mut conductor = Conductor::new();
        conductor.force_bpm(Some(bpm));

        let strum_y = if config.is_downscroll {
            VIRTUAL_HEIGHT - STRUMLINE_SIZE - STRUMLINE_Y_OFFSET
        } else {
            STRUMLINE_Y_OFFSET
        };

        let mut player_strumline = Strumline::new(
            true,
            config.is_downscroll,
            config.scroll_speed,
            VIRTUAL_WIDTH * 0.5 + STRUMLINE_X_OFFSET,
            strum_y,
        );
        let mut opponent_strumline = Strumline::new(
            false,
            config.is_downscroll,
            config.scroll_speed,
            STRUMLINE_X_OFFSET,
            strum_y,
        );

        player_strumline.apply_note_data(player_notes);
        opponent_strumline.apply_note_data(opponent_notes);

        let tallies = ScoreTallies {
            total_notes: player_notes.len() as u32,
            ..Default::default()
        };

        let mut session = Self {
            config,
            conductor,
            player_strumline,
            opponent_strumline,
            popups: PopUpPool::default(),
            tallies,
            song_score_f64: 0.0,
            health: HEALTH_STARTING,
            health_lerp: HEALTH_STARTING,
            is_paused: false,
            is_player_dying: false,
            sfx_slots: GameplaySfxSlots::default(),
            hide_opponent_strumline: false,
            hide_popups: false,
            score_text_buf: [0u8; 32],
            score_text_len: 0,
            strum_hitbox_height: STRUMLINE_SIZE,
            strum_offset_y: 0.0,
            rng_seed: 0x1337_BEEF,
        };
        session.refresh_hud_buffers();
        session
    }

    /// Current integer song score (`Math.round` / truncation matching HUD display).
    #[inline]
    #[must_use]
    pub fn score(&self) -> i64 {
        self.song_score_f64.round() as i64
    }

    /// Sets loaded note-style strumline hitbox height and vertical offset and
    /// repositions both strumlines cleanly (`PlayState.initStrumlines`).
    pub fn set_strumline_metrics(&mut self, hitbox_height: f32, offset_y: f32) {
        self.strum_hitbox_height = hitbox_height.max(1.0);
        self.strum_offset_y = offset_y;
        self.reposition_strumlines();
    }

    /// Toggles or sets downscroll and repositions both strumlines.
    pub fn set_downscroll(&mut self, is_downscroll: bool) {
        self.config.is_downscroll = is_downscroll;
        self.reposition_strumlines();
    }

    fn reposition_strumlines(&mut self) {
        let strum_y = if self.config.is_downscroll {
            VIRTUAL_HEIGHT - self.strum_hitbox_height - STRUMLINE_Y_OFFSET - self.strum_offset_y
        } else {
            STRUMLINE_Y_OFFSET
        };
        self.player_strumline.is_downscroll = self.config.is_downscroll;
        self.player_strumline.y = strum_y;
        self.player_strumline.layout_receptors();

        self.opponent_strumline.is_downscroll = self.config.is_downscroll;
        self.opponent_strumline.y = strum_y;
        self.opponent_strumline.layout_receptors();
    }

    /// Discards notes skipped when seeking forward/backward (`PlayState.handleSkippedNotes`).
    pub fn handle_skipped_notes(&mut self) {
        self.player_strumline.handle_skipped_notes();
        self.opponent_strumline.handle_skipped_notes();
    }

    /// Runs one gameplay simulation step with **zero heap allocations**.
    pub fn update(
        &mut self,
        raw_song_pos_ms: f64,
        elapsed_s: f32,
        frame_timestamp: Instant,
        inputs: &mut InputQueue,
        mut audio: Option<&mut AudioEngine>,
        style: &StrumlineVisualStyle,
    ) -> PlayStepEvents {
        let mut step_events = PlayStepEvents::default();
        if self.is_paused {
            inputs.clear();
            return step_events;
        }

        step_events.conductor = self.conductor.update(raw_song_pos_ms, true);
        let song_pos = self.conductor.song_position_ms;
        let time_with_delta = self.conductor.time_with_delta_ms();

        // 1. Update strumlines (note spawning, window classification, hold clipping).
        let opp_events = self.opponent_strumline.update(
            elapsed_s,
            song_pos,
            time_with_delta,
            self.config.start_timestamp_ms,
            true,
            style,
        );
        for hit_h in opp_events
            .bot_hit_handles
            .iter()
            .take(opp_events.bot_hit_len)
            .flatten()
        {
            if let Some(note) = self.opponent_strumline.notes.get(*hit_h) {
                step_events.opponent_hit_dir = Some(note.direction);
                step_events.opponent_hit_kind = note.kind;
                let hold_h = note.hold_handle;
                self.opponent_strumline.hit_note(*hit_h, true, song_pos);
                if let Some(hh) = hold_h {
                    self.opponent_strumline.play_note_hold_cover(hh, true);
                }
            }
        }

        let player_events = self.player_strumline.update(
            elapsed_s,
            song_pos,
            time_with_delta,
            self.config.start_timestamp_ms,
            self.config.is_botplay,
            style,
        );

        // Botplay hits on player strumline.
        for hit_h in player_events
            .bot_hit_handles
            .iter()
            .take(player_events.bot_hit_len)
            .flatten()
        {
            if let Some(note) = self.player_strumline.notes.get(*hit_h) {
                step_events.player_hit_dir = Some(note.direction);
                step_events.player_hit_kind = note.kind;
                let hold_h = note.hold_handle;
                self.player_strumline.hit_note(*hit_h, true, song_pos);
                if let Some(hh) = hold_h {
                    self.player_strumline.play_note_hold_cover(hh, true);
                }
            }
        }

        // Player note misses past HIT_WINDOW_MS.
        for miss_h in player_events
            .newly_missed_handles
            .iter()
            .take(player_events.newly_missed_len)
            .flatten()
        {
            let mut miss_dir = None;
            if let Some(note) = self.player_strumline.notes.get_mut(*miss_h) {
                note.handled_miss = true;
                miss_dir = Some(note.direction);
            }
            if !self.config.is_botplay {
                self.on_note_miss(audio.as_deref_mut());
                step_events.missed_note = true;
                if miss_dir.is_some() {
                    step_events.missed_dir = miss_dir;
                }
            }
        }

        // Continuous sustain hold bonus tick.
        if !self.config.is_botplay && player_events.active_hold_scoreable_count > 0 {
            let bonus_count = player_events.active_hold_scoreable_count as f32;
            self.health += HEALTH_HOLD_BONUS_PER_SECOND * elapsed_s * bonus_count;
            self.song_score_f64 +=
                f64::from(SCORE_HOLD_BONUS_PER_SECOND) * f64::from(elapsed_s) * f64::from(bonus_count);
        }

        // Player early hold-drop penalty.
        for drop_h in player_events
            .hold_drop_handles
            .iter()
            .take(player_events.hold_drop_len)
            .flatten()
        {
            if let Some(hold) = self.player_strumline.holds.get_mut(*drop_h) {
                hold.handled_miss = true;
                let hold_dir = hold.direction;
                if !self.config.is_botplay && hold.scoreable {
                    let penalty = compute_hold_drop_penalty(hold.sustain_ms, hold.hit_note);
                    if penalty.should_penalize {
                        self.apply_score(
                            penalty.score_change,
                            None,
                            penalty.health_change,
                            true,
                        );
                        self.play_miss_audio(audio.as_deref_mut(), 0.55);
                        step_events.missed_note = true;
                        step_events.missed_dir = Some(hold_dir);
                    }
                }
            }
        }

        // 2. Process buffered press/release inputs with timestamp latency compensation.
        self.process_input_queue(
            frame_timestamp,
            inputs,
            audio.as_deref_mut(),
            style,
            &mut step_events,
        );

        // 3. Clamp health, update lerp, popups, and HUD text stack buffers.
        self.health = self.health.clamp(HEALTH_MIN, HEALTH_MAX);
        self.health_lerp = if self.config.is_botplay {
            HEALTH_MAX
        } else {
            self.health_lerp + (self.health - self.health_lerp) * 0.15
        };

        if self.health <= HEALTH_MIN && !self.config.practice_mode && !self.is_player_dying {
            self.is_player_dying = true;
            step_events.died = true;
            if let Some(engine) = audio {
                let _ = engine.send(AudioCommand::Pause);
            }
        }

        self.popups.update(elapsed_s);
        self.refresh_hud_buffers();
        step_events
    }

    fn process_input_queue(
        &mut self,
        frame_timestamp: Instant,
        inputs: &mut InputQueue,
        mut audio: Option<&mut AudioEngine>,
        style: &StrumlineVisualStyle,
        step_events: &mut PlayStepEvents,
    ) {
        if inputs.total_len() == 0 {
            return;
        }

        let (mut buckets, mut counts) = self.player_strumline.collect_notes_may_hit();
        let any_notes_in_range = counts.iter().any(|&c| c > 0);

        while let Some(ev) = inputs.presses.pop_front() {
            self.player_strumline.press_key(ev.direction, ev.key_code);
            if self.config.is_botplay {
                continue;
            }

            let lane = ev.direction.as_index();
            let lane_count = counts[lane];

            if lane_count == 0 {
                if !self.config.ghost_tapping || !self.player_strumline.may_ghost_tap() {
                    let _ = any_notes_in_range;
                    self.ghost_note_miss(audio.as_deref_mut());
                    step_events.ghost_missed = true;
                    step_events.missed_dir = Some(ev.direction);
                }
                self.player_strumline.play_press(ev.direction);
                continue;
            }

            // Pick earliest non-low-priority note in the lane (falling back to index 0).
            let mut chosen_slot = 0usize;
            let mut best_time = f64::INFINITY;
            let mut found_normal_priority = false;

            for (i, slot) in buckets[lane].iter().take(lane_count).enumerate() {
                if let Some(handle) = *slot {
                    if let Some(note) = self.player_strumline.notes.get(handle) {
                        if !note.low_priority && (!found_normal_priority || note.strum_time_ms < best_time) {
                            found_normal_priority = true;
                            best_time = note.strum_time_ms;
                            chosen_slot = i;
                        } else if !found_normal_priority && note.strum_time_ms < best_time {
                            best_time = note.strum_time_ms;
                            chosen_slot = i;
                        }
                    }
                }
            }

            let Some(target_handle) = buckets[lane][chosen_slot] else {
                continue;
            };
            // Remove from local stack bucket without allocation.
            buckets[lane][chosen_slot] = buckets[lane][lane_count - 1];
            buckets[lane][lane_count - 1] = None;
            counts[lane] -= 1;

            self.good_note_hit(
                target_handle,
                ev.timestamp,
                frame_timestamp,
                audio.as_deref_mut(),
                style,
                step_events,
            );
            self.player_strumline.play_confirm(ev.direction);
        }

        while let Some(ev) = inputs.releases.pop_front() {
            self.player_strumline.release_key(ev.direction, Some(ev.key_code));
            if !self.player_strumline.is_key_held(ev.direction) {
                self.player_strumline.play_static(ev.direction);
            }
        }
    }

    fn good_note_hit(
        &mut self,
        note_handle: PoolHandle,
        event_timestamp: Instant,
        frame_timestamp: Instant,
        audio: Option<&mut AudioEngine>,
        style: &StrumlineVisualStyle,
        step_events: &mut PlayStepEvents,
    ) {
        let Some((note_time_ms, direction, kind, scoreable, hold_handle)) = self
            .player_strumline
            .notes
            .get(note_handle)
            .map(|n| (n.strum_time_ms, n.direction, n.kind, n.scoreable, n.hold_handle))
        else {
            return;
        };

        let note_diff_ms = compute_note_diff_ms(
            self.conductor.song_position_ms,
            note_time_ms,
            event_timestamp,
            frame_timestamp,
        );
        let truncated_diff = note_diff_ms.trunc();

        let points = f64::from(score_note(truncated_diff, self.config.scoring_system));
        let judgement = judge_note(truncated_diff, self.config.scoring_system);
        let health_change = judgement.health_bonus();
        let breaks_combo = judgement.breaks_combo();

        self.player_strumline
            .hit_note(note_handle, !breaks_combo, self.conductor.song_position_ms);

        if judgement == Judgement::Sick {
            self.player_strumline
                .play_note_splash(direction, style.show_splashes);
        }
        if let Some(hold_h) = hold_handle {
            self.player_strumline.play_note_hold_cover(hold_h, true);
        }

        if let Some(engine) = audio {
            let _ = engine.send(AudioCommand::SetTrackGain {
                track: TrackKind::PlayerVocals,
                gain: self.config.player_vocals_gain,
            });
        }

        if scoreable {
            self.tallies.total_notes_hit += 1;
            self.apply_score(points, Some(judgement), health_change, breaks_combo);
            let j_idx = match judgement {
                Judgement::Sick => 0,
                Judgement::Good => 1,
                Judgement::Bad => 2,
                Judgement::Shit => 3,
                Judgement::Miss => 0,
            };
            self.popups.spawn_rating(
                judgement,
                style.popup_rating_sizes[j_idx],
                style.popup_rating_offsets[j_idx],
                self.conductor.beat_length_ms(),
                style.popup_rating_pixel[j_idx] || style.is_pixel,
            );
            self.popups.spawn_combo(
                self.tallies.combo,
                style.popup_digit_offsets[0],
                self.conductor.beat_length_ms(),
                style.popup_digit_pixel[0] || style.is_pixel,
            );
        }

        step_events.last_judgement = Some(judgement);
        step_events.last_note_diff_ms = Some(truncated_diff);
        step_events.player_hit_dir = Some(direction);
        step_events.player_hit_kind = kind;
    }

    fn on_note_miss(&mut self, audio: Option<&mut AudioEngine>) {
        let miss_score = f64::from(get_miss_score(self.config.scoring_system));
        self.apply_score(
            miss_score,
            Some(Judgement::Miss),
            HEALTH_MISS_PENALTY,
            true,
        );
        self.play_miss_audio(audio, 0.55);
    }

    fn ghost_note_miss(&mut self, audio: Option<&mut AudioEngine>) {
        self.health += fnfrs_core::constants::HEALTH_GHOST_MISS_PENALTY;
        self.song_score_f64 += f64::from(SCORE_GHOST_MISS_PENALTY);
        self.play_miss_audio(audio, 0.15);
    }

    fn play_miss_audio(&mut self, audio: Option<&mut AudioEngine>, gain: f32) {
        let Some(engine) = audio else {
            return;
        };
        let _ = engine.send(AudioCommand::SetTrackGain {
            track: TrackKind::PlayerVocals,
            gain: 0.0,
        });
        let idx = (self.next_rng() as usize) % self.sfx_slots.miss_notes.len();
        if let Some(slot) = self.sfx_slots.miss_notes[idx] {
            let _ = engine.send(AudioCommand::PlaySfx { slot, gain });
        }
    }

    fn apply_score(
        &mut self,
        score_delta: f64,
        judgement: Option<Judgement>,
        health_delta: f32,
        breaks_combo: bool,
    ) {
        self.song_score_f64 += score_delta;
        self.health = (self.health + health_delta).clamp(HEALTH_MIN, HEALTH_MAX);

        if breaks_combo {
            self.tallies.combo = 0;
        } else {
            self.tallies.combo += 1;
            self.tallies.max_combo = self.tallies.max_combo.max(self.tallies.combo);
        }

        if let Some(j) = judgement {
            match j {
                Judgement::Sick => self.tallies.sick += 1,
                Judgement::Good => self.tallies.good += 1,
                Judgement::Bad => self.tallies.bad += 1,
                Judgement::Shit => self.tallies.shit += 1,
                Judgement::Miss => self.tallies.missed += 1,
            }
        }
    }

    fn next_rng(&mut self) -> u32 {
        let mut x = self.rng_seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_seed = x;
        x
    }

    /// Refreshes stack-formatted HUD score text immediately (e.g. after toggling Botplay).
    pub fn refresh_hud(&mut self) {
        self.refresh_hud_buffers();
    }

    fn refresh_hud_buffers(&mut self) {
        let score = self.score();
        let formatted = format_score_into(&mut self.score_text_buf, score, self.config.is_botplay);
        self.score_text_len = formatted.len();
    }

    /// Zero-allocation formatted HUD score bytes (`"Score: 1,234,560"` or `"Bot Play Enabled"`).
    #[must_use]
    pub fn score_text_bytes(&self) -> &[u8] {
        &self.score_text_buf[..self.score_text_len]
    }

    /// Queues strumlines, popups, health bar, and HUD text across depth-ordered
    /// batches (`bg_sprites`, `mesh_batcher`, `fg_sprites`) with zero heap allocations.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        bg_sprites: &mut SpriteBatcher,
        fg_sprites: &mut SpriteBatcher,
        mesh_batcher: &mut MeshBatcher,
        style: &StrumlineVisualStyle,
        note_assets: Option<&NoteStyleAssets>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        let song_pos = self.conductor.song_position_ms;
        if !self.hide_opponent_strumline {
            self.opponent_strumline
                .draw(bg_sprites, fg_sprites, mesh_batcher, style, note_assets, song_pos);
        }
        self.player_strumline
            .draw(bg_sprites, fg_sprites, mesh_batcher, style, note_assets, song_pos);

        // Judgement & combo popups (real V-Slice `ui/popup/*` PNG sprites, falling back to text).
        if !self.hide_popups {
            for (_, rating) in self.popups.ratings.iter_alive() {
            let drawn_sprite = note_assets
                .and_then(|na| na.popup_rating_sprite(rating.judgement))
                .and_then(|asset| asset.tex.map(|tex| (asset, tex)));
            if let Some((asset, (tex, tex_sz))) = drawn_sprite {
                let sz = [tex_sz[0] * asset.scale, tex_sz[1] * asset.scale];
                let pos = if asset.is_pixel || rating.is_pixel {
                    [rating.x.round(), rating.y.round()]
                } else {
                    [rating.x, rating.y]
                };
                let filter = if asset.is_pixel || rating.is_pixel {
                    fnfrs_render::FilterMode::Nearest
                } else {
                    fnfrs_render::FilterMode::Linear
                };
                fg_sprites.push(
                    tex,
                    fnfrs_render::BlendMode::Normal,
                    filter,
                    fnfrs_render::SpriteInstance::new(pos, sz)
                        .with_color([1.0, 1.0, 1.0, rating.alpha * asset.alpha]),
                );
            } else {
                let (label, color): (&str, [f32; 4]) = match rating.judgement {
                    Judgement::Sick => ("SICK!", [0.22, 0.94, 1.0, rating.alpha]),
                    Judgement::Good => ("GOOD!", [0.35, 0.95, 0.45, rating.alpha]),
                    Judgement::Bad => ("BAD", [1.0, 0.72, 0.25, rating.alpha]),
                    Judgement::Shit => ("SHIT", [0.85, 0.30, 0.30, rating.alpha]),
                    Judgement::Miss => ("MISS", [0.75, 0.15, 0.20, rating.alpha]),
                };
                font.draw_bytes_outlined(
                    fg_sprites,
                    font_tex,
                    label.as_bytes(),
                    [(rating.x + 80.0).round(), (rating.y + 30.0).round()],
                    &TextStyle {
                        scale: if font.line_height() <= 10.0 { 4.0 } else { 2.0 },
                        color,
                        ..Default::default()
                    },
                    [0.0, 0.0, 0.0, 1.0],
                );
            }
        }

        let mut digit_byte = [b'0'; 1];
        for (_, digit) in self.popups.digits.iter_alive() {
            let drawn_sprite = note_assets
                .and_then(|na| na.popup_digit_sprite(digit.digit))
                .and_then(|asset| asset.tex.map(|tex| (asset, tex)));
            if let Some((asset, (tex, tex_sz))) = drawn_sprite {
                let sz = [tex_sz[0] * asset.scale, tex_sz[1] * asset.scale];
                let pos = if asset.is_pixel || digit.is_pixel {
                    [digit.x.round(), digit.y.round()]
                } else {
                    [digit.x, digit.y]
                };
                let filter = if asset.is_pixel || digit.is_pixel {
                    fnfrs_render::FilterMode::Nearest
                } else {
                    fnfrs_render::FilterMode::Linear
                };
                fg_sprites.push(
                    tex,
                    fnfrs_render::BlendMode::Normal,
                    filter,
                    fnfrs_render::SpriteInstance::new(pos, sz)
                        .with_color([1.0, 1.0, 1.0, digit.alpha * asset.alpha]),
                );
            } else {
                digit_byte[0] = b'0' + (digit.digit % 10);
                font.draw_bytes_outlined(
                    fg_sprites,
                    font_tex,
                    &digit_byte,
                    [digit.x.round(), digit.y.round()],
                    &TextStyle {
                        scale: if font.line_height() <= 10.0 { 3.5 } else { 1.75 },
                        color: [1.0, 1.0, 1.0, digit.alpha],
                        ..Default::default()
                    },
                    [0.0, 0.0, 0.0, 1.0],
                );
            }
        }
        }

        // V-Slice health bar (`PlayState.initHealthBar`, `Constants.COLOR_HEALTH_BAR_*`).
        let (bg_w, bg_h) = note_assets
            .and_then(|na| na.health_bar_tex)
            .map_or((601.0, 19.0), |(_, sz)| (sz[0], sz[1]));
        let bar_bg_x = (VIRTUAL_WIDTH - bg_w) * 0.5;
        let bar_bg_y = if self.config.is_downscroll {
            VIRTUAL_HEIGHT * 0.1
        } else {
            VIRTUAL_HEIGHT * 0.9
        };
        let bar_x = bar_bg_x + 4.0;
        let bar_y = bar_bg_y + 4.0;
        let bar_w = (bg_w - 8.0).max(1.0);
        let bar_h = (bg_h - 8.0).max(1.0);

        let player_ratio = (self.health_lerp / HEALTH_MAX).clamp(0.0, 1.0);
        let split_x = bar_w * (1.0 - player_ratio);

        if let Some((hb_tex, _)) = note_assets.and_then(|na| na.health_bar_tex) {
            fg_sprites.push(
                hb_tex,
                fnfrs_render::BlendMode::Normal,
                fnfrs_render::FilterMode::Linear,
                fnfrs_render::SpriteInstance::new([bar_bg_x, bar_bg_y], [bg_w, bg_h]),
            );
        } else {
            fg_sprites.push(
                style.white_texture,
                fnfrs_render::BlendMode::Normal,
                fnfrs_render::FilterMode::Nearest,
                fnfrs_render::SpriteInstance::new([bar_bg_x, bar_bg_y], [bg_w, bg_h])
                    .with_color([0.0, 0.0, 0.0, 1.0]),
            );
        }

        // Opponent left fill (`#FF0000`), player right fill (`#66FF33`).
        fg_sprites.push(
            style.white_texture,
            fnfrs_render::BlendMode::Normal,
            fnfrs_render::FilterMode::Nearest,
            fnfrs_render::SpriteInstance::new([bar_x, bar_y], [split_x, bar_h])
                .with_color([1.0, 0.0, 0.0, 1.0]),
        );
        fg_sprites.push(
            style.white_texture,
            fnfrs_render::BlendMode::Normal,
            fnfrs_render::FilterMode::Nearest,
            fnfrs_render::SpriteInstance::new([bar_x + split_x, bar_y], [bar_w - split_x, bar_h])
                .with_color([0.4, 1.0, 0.2, 1.0]),
        );

        let score_scale = if font.line_height() <= 10.0 { 2.0 } else { 1.0 };
        let hud_style = TextStyle {
            scale: score_scale,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Left,
            letter_spacing: 0.0,
            ..Default::default()
        };
        font.draw_bytes_outlined(
            fg_sprites,
            font_tex,
            self.score_text_bytes(),
            [(bar_bg_x + bg_w - 190.0).round(), (bar_bg_y + 30.0).round()],
            &hud_style,
            [0.0, 0.0, 0.0, 1.0],
        );
    }

    /// Returns `true` if the player currently has an active, unmissed sustain trail held.
    #[must_use]
    pub fn is_player_holding_sustain(&self) -> bool {
        self.player_strumline
            .holds
            .iter_alive()
            .any(|(_, h)| h.hit_note && !h.missed_note && h.sustain_ms > 0.0)
    }

    /// Returns `true` if any player lane key is currently held.
    #[must_use]
    pub fn is_player_holding_key(&self) -> bool {
        NoteDirection::ALL
            .iter()
            .any(|&dir| self.player_strumline.is_key_held(dir))
    }

    /// Returns `true` if the opponent currently has an active sustain trail held.
    #[must_use]
    pub fn is_opponent_holding_sustain(&self) -> bool {
        self.opponent_strumline
            .holds
            .iter_alive()
            .any(|(_, h)| h.hit_note && !h.missed_note && h.sustain_ms > 0.0)
    }
}

/// Initialization parameters for [`PlayScene`].
#[derive(Debug, Clone)]
pub struct PlaySceneConfig {
    pub song_name: String,
    pub difficulty: String,
    pub start_countdown: bool,
    pub song_duration_ms: Option<f64>,
    pub death_count: u32,
}

impl Default for PlaySceneConfig {
    fn default() -> Self {
        Self {
            song_name: "Unknown".to_owned(),
            difficulty: "normal".to_owned(),
            start_countdown: true,
            song_duration_ms: None,
            death_count: 0,
        }
    }
}

/// Per-frame events emitted by [`PlayScene::update`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaySceneStepEvents {
    pub gameplay: PlayStepEvents,
    pub countdown_step: Option<CountdownStep>,
    pub countdown_sfx: Option<CountdownSfx>,
    pub countdown_finished_now: bool,
    pub game_over_events: GameOverEvents,
    pub song_finished_now: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ScrollSpeedTween {
    active: bool,
    start: f64,
    target: f64,
    elapsed_s: f32,
    duration_s: f32,
    ease: FlxEase,
}

impl ScrollSpeedTween {
    fn start(&mut self, current: f64, target: f64, duration_s: f32, ease: FlxEase) -> Option<f64> {
        if duration_s <= 0.0 || ease == FlxEase::Instant {
            self.active = false;
            return Some(target);
        }
        self.active = true;
        self.start = current;
        self.target = target;
        self.elapsed_s = 0.0;
        self.duration_s = duration_s;
        self.ease = ease;
        None
    }

    fn update(&mut self, elapsed_s: f32) -> Option<f64> {
        if !self.active {
            return None;
        }
        self.elapsed_s += elapsed_s.max(0.0);
        let raw_t = if self.duration_s > 0.0 {
            (self.elapsed_s / self.duration_s).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let eased = f64::from(self.ease.eval(raw_t));
        let value = self.start + (self.target - self.start) * eased;
        if raw_t >= 1.0 {
            self.active = false;
            return Some(self.target);
        }
        Some(value)
    }
}

/// Complete V-Slice gameplay scene coordinating [`PlaySession`], [`Stage`],
/// [`BaseCharacter`]s, [`CameraRig`], [`SongEventQueue`], [`HealthIcon`]s,
/// [`CountdownController`], [`PauseMenuState`], [`GameOverState`], and
/// [`ResultsSummaryState`] with **zero per-frame heap allocations**.
#[derive(Debug)]
pub struct PlayScene {
    pub session: PlaySession,
    pub stage: Option<Stage>,
    pub bf: Option<BaseCharacter>,
    pub dad: Option<BaseCharacter>,
    pub gf: Option<BaseCharacter>,
    pub camera_rig: CameraRig,
    pub event_queue: SongEventQueue,
    pub icon_p1: HealthIcon,
    pub icon_p2: HealthIcon,
    pub countdown: CountdownController,
    pub countdown_assets: Option<CountdownAssets>,
    pub pause_menu: PauseMenuState,
    pub game_over: GameOverState,
    pub results: ResultsSummaryState,
    pub scene_config: PlaySceneConfig,
    pub scripts: NativeScriptHost,

    base_scroll_speed: f64,
    health_bar_size: [f32; 2],
    player_scroll_tween: ScrollSpeedTween,
    opponent_scroll_tween: ScrollSpeedTween,
}

impl PlayScene {
    /// Builds a [`PlayScene`], places characters on `stage`, initializes cameras,
    /// and optionally starts the 4-beat pre-roll countdown.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        session: PlaySession,
        mut stage: Option<Stage>,
        mut bf: Option<BaseCharacter>,
        mut dad: Option<BaseCharacter>,
        mut gf: Option<BaseCharacter>,
        event_queue: SongEventQueue,
        icon_p1: HealthIcon,
        icon_p2: HealthIcon,
        countdown_assets: Option<CountdownAssets>,
        scene_config: PlaySceneConfig,
    ) -> Self {
        if let Some(st) = &mut stage {
            st.reset_stage(bf.as_mut(), dad.as_mut(), gf.as_mut());
        }
        if let Some(player) = &mut bf {
            player.is_dead = false;
        }

        let stage_zoom = stage.as_ref().map_or(DEFAULT_STAGE_ZOOM, |s| s.camera_zoom);
        let mut camera_rig = CameraRig::new(stage_zoom);
        let initial_focus = initial_camera_focus(bf.as_ref(), dad.as_ref(), gf.as_ref());
        camera_rig.cam_game.snap_to(initial_focus[0], initial_focus[1]);

        let pause_menu = PauseMenuState::new(&scene_config.song_name, &scene_config.difficulty);
        let mut game_over = GameOverState::new();
        game_over.death_count = scene_config.death_count;
        let bf_is_pixel = bf.as_ref().is_some_and(|c| c.is_pixel);
        game_over.set_suffix(if bf_is_pixel { "-pixel" } else { "" }, bf_is_pixel);

        let beat_ms = session.conductor.beat_length_ms();
        let mut countdown = CountdownController::new(beat_ms);
        if scene_config.start_countdown {
            countdown.start_at(beat_ms, session.config.start_timestamp_ms);
        } else {
            countdown.skip();
        }

        let base_scroll_speed = session.config.scroll_speed.max(0.1);

        Self {
            session,
            stage,
            bf,
            dad,
            gf,
            camera_rig,
            event_queue,
            icon_p1,
            icon_p2,
            countdown,
            countdown_assets,
            pause_menu,
            game_over,
            results: ResultsSummaryState::new(),
            scene_config,
            scripts: NativeScriptHost::empty(),
            base_scroll_speed,
            health_bar_size: [601.0, 19.0],
            player_scroll_tween: ScrollSpeedTween::default(),
            opponent_scroll_tween: ScrollSpeedTween::default(),
        }
    }

    /// Attaches a populated [`NativeScriptHost`] and runs initial stage setup hooks.
    pub fn attach_scripts(&mut self, mut scripts: NativeScriptHost) {
        if let Some(dur) = self.scene_config.song_duration_ms {
            scripts.set_song_duration_ms(dur);
        }
        self.session.hide_opponent_strumline = scripts.hides_opponent_strumline();
        self.session.hide_popups = scripts.hides_popups();
        if scripts.centers_player_strumline() {
            self.session.player_strumline.center_horizontally();
        }
        scripts.on_stage_ready(
            self.stage.as_mut(),
            self.bf.as_mut(),
            self.dad.as_mut(),
            self.gf.as_mut(),
        );
        self.scripts = scripts;
    }

    /// Updates cached health bar dimensions used to anchor [`HealthIcon`]s.
    pub fn set_health_bar_size(&mut self, width: f32, height: f32) {
        self.health_bar_size = [width.max(10.0), height.max(10.0)];
    }

    /// Resets scene state for song restart or difficulty reload without reallocating pools.
    pub fn restart(
        &mut self,
        new_session: PlaySession,
        song_name: &str,
        difficulty: &str,
        song_duration_ms: Option<f64>,
        reset_death_count: bool,
    ) {
        self.session = new_session;
        self.scene_config.song_name.clear();
        self.scene_config.song_name.push_str(song_name);
        self.scene_config.difficulty.clear();
        self.scene_config.difficulty.push_str(difficulty);
        self.scene_config.song_duration_ms = song_duration_ms;

        self.base_scroll_speed = self.session.config.scroll_speed.max(0.1);
        self.player_scroll_tween = ScrollSpeedTween::default();
        self.opponent_scroll_tween = ScrollSpeedTween::default();
        self.event_queue.reset_to_time_ms(0.0);

        if let Some(st) = &mut self.stage {
            st.reset_stage(self.bf.as_mut(), self.dad.as_mut(), self.gf.as_mut());
        } else {
            if let Some(c) = &mut self.bf {
                c.reset_character(true);
            }
            if let Some(c) = &mut self.dad {
                c.reset_character(true);
            }
            if let Some(c) = &mut self.gf {
                c.reset_character(true);
            }
        }
        self.scripts.reset(self.stage.as_mut());
        if let Some(player) = &mut self.bf {
            player.is_dead = false;
        }

        self.camera_rig.reset();
        let focus = initial_camera_focus(self.bf.as_ref(), self.dad.as_ref(), self.gf.as_ref());
        self.camera_rig.cam_game.snap_to(focus[0], focus[1]);

        self.pause_menu.set_song_info(song_name, difficulty);
        self.pause_menu.close();
        self.game_over.reset();
        if reset_death_count {
            self.game_over.death_count = 0;
        }
        self.results.hide();

        let beat_ms = self.session.conductor.beat_length_ms();
        if self.scene_config.start_countdown {
            self.countdown
                .start_at(beat_ms, self.session.config.start_timestamp_ms);
        } else {
            self.countdown.skip();
        }
    }

    /// Advances gameplay, countdown, song events, characters, stage, camera rig,
    /// and health icons for one frame with **zero heap allocations**.
    pub fn update(
        &mut self,
        raw_song_pos_ms: f64,
        elapsed_s: f32,
        frame_timestamp: Instant,
        inputs: &mut InputQueue,
        mut audio: Option<&mut AudioEngine>,
        style: &StrumlineVisualStyle,
    ) -> PlaySceneStepEvents {
        let mut out = PlaySceneStepEvents::default();

        if self.game_over.phase != GameOverPhase::Inactive {
            inputs.clear();
            self.camera_rig.update(elapsed_s);
            if let Some(bf) = &mut self.bf {
                let target_anim = match self.game_over.phase {
                    GameOverPhase::FirstDeath => "firstDeath",
                    GameOverPhase::DeathLoop => "deathLoop",
                    GameOverPhase::ConfirmRetry | GameOverPhase::Exiting => "deathConfirm",
                    GameOverPhase::Inactive => "firstDeath",
                };
                if bf.current_animation() != target_anim && bf.has_animation(target_anim) {
                    bf.play_animation(target_anim, true, true);
                }
                bf.update(elapsed_s, self.session.conductor.beat_length_ms(), false);
            }
            return out;
        }

        if self.pause_menu.is_open || self.results.is_active || self.session.is_paused {
            inputs.clear();
            return out;
        }

        let effective_song_pos_ms = if self.countdown.active {
            let is_pixel = style.is_pixel
                || self
                    .countdown_assets
                    .as_ref()
                    .is_some_and(|a| a.default_pixel);
            let tick = self.countdown.update(elapsed_s, is_pixel);
            out.countdown_step = tick.entered_step;
            out.countdown_sfx = tick.sfx;
            out.countdown_finished_now = tick.finished_now;

            if let Some(sfx) = tick.sfx {
                if let Some(slot) = self
                    .countdown_assets
                    .as_ref()
                    .and_then(|a| a.sfx_slots[sfx.as_index()])
                {
                    if let Some(ref mut engine) = audio {
                        let _ = engine.send(AudioCommand::PlaySfx {
                            slot,
                            gain: COUNTDOWN_VOLUME,
                        });
                    }
                }
            }

            if let Some(step) = tick.entered_step {
                let is_countdown_beat = matches!(
                    step,
                    CountdownStep::Three
                        | CountdownStep::Two
                        | CountdownStep::One
                        | CountdownStep::Go
                );
                if is_countdown_beat {
                    self.icon_p1.bop();
                    self.icon_p2.bop();
                }
            }

            if tick.finished_now {
                if let Some(ref mut engine) = audio {
                    let _ = engine.play();
                }
                self.session.config.start_timestamp_ms
            } else {
                self.countdown.song_time_ms
            }
        } else {
            raw_song_pos_ms
        };

        let gameplay = self.session.update(
            effective_song_pos_ms,
            elapsed_s,
            frame_timestamp,
            inputs,
            audio.as_deref_mut(),
            style,
        );
        out.gameplay = gameplay;

        if gameplay.died {
            self.session.is_paused = true;
            if let Some(bf) = &mut self.bf {
                bf.is_dead = true;
                bf.play_animation("firstDeath", true, true);
                let focus = bf.get_camera_focus_point();
                self.camera_rig.cam_game.set_target(focus[0], focus[1]);
                self.camera_rig.cam_game.default_zoom = 1.05;
            }
            out.game_over_events = self.game_over.trigger_death();
            return out;
        }

        let song_pos = self.session.conductor.song_position_ms;
        if !self.countdown.active || song_pos >= 0.0 {
            let step_len_ms = self.session.conductor.step_length_ms();
            let base_scroll = self.base_scroll_speed;
            let camera_rig = &mut self.camera_rig;
            let bf = &mut self.bf;
            let dad = &mut self.dad;
            let gf = &mut self.gf;
            let stage = &mut self.stage;
            let player_strumline = &mut self.session.player_strumline;
            let opponent_strumline = &mut self.session.opponent_strumline;
            let player_tween = &mut self.player_scroll_tween;
            let opponent_tween = &mut self.opponent_scroll_tween;
            let scripts = &mut self.scripts;

            let mut pending_flash_dur: Option<f32> = None;
            let mut event_cues = crate::scripts::ScriptStepOutput::default();
            self.event_queue.update(song_pos, |ev| {
                if let GameEventAction::FocusCamera { target, .. } = &ev.action {
                    scripts.on_focus_character(
                        *target == CameraFocusTarget::Player,
                        *target == CameraFocusTarget::Opponent,
                    );
                } else if let GameEventAction::Custom { event_kind, value } = &ev.action {
                    let c = scripts.on_song_event(
                        event_kind,
                        value,
                        stage.as_mut(),
                        bf.as_mut(),
                        dad.as_mut(),
                        &mut pending_flash_dur,
                    );
                    for cue in c.cues.iter().take(c.count).flatten() {
                        event_cues.push(cue.slot, cue.gain);
                    }
                }
                apply_song_event(
                    ev,
                    step_len_ms,
                    base_scroll,
                    camera_rig,
                    bf.as_mut(),
                    dad.as_mut(),
                    gf.as_mut(),
                    stage.as_mut(),
                    player_strumline,
                    opponent_strumline,
                    player_tween,
                    opponent_tween,
                );
            });
            if let Some(dur) = pending_flash_dur {
                self.camera_rig.cam_game.flash([1.0, 1.0, 1.0, 1.0], dur);
            }
            if let Some(ref mut engine) = audio {
                for cue in event_cues.cues.iter().take(event_cues.count).flatten() {
                    let _ = engine.send(AudioCommand::PlaySfx {
                        slot: cue.slot,
                        gain: cue.gain,
                    });
                }
            }
        }

        if let Some(speed) = self.player_scroll_tween.update(elapsed_s) {
            self.session.player_strumline.scroll_speed = speed.max(0.1);
        }
        if let Some(speed) = self.opponent_scroll_tween.update(elapsed_s) {
            self.session.opponent_strumline.scroll_speed = speed.max(0.1);
        }

        if let Some(dir) = gameplay.player_hit_dir {
            self.scripts.on_note_hit(
                true,
                dir,
                gameplay.player_hit_kind,
                self.bf.as_mut(),
                self.stage.as_ref(),
            );
        } else if gameplay.missed_note || gameplay.ghost_missed {
            if let Some(dir) = gameplay.missed_dir {
                if let Some(bf) = &mut self.bf {
                    bf.play_sing_animation(dir, true, "");
                }
            }
        }

        if let Some(dir) = gameplay.opponent_hit_dir {
            self.scripts.on_note_hit(
                false,
                dir,
                gameplay.opponent_hit_kind,
                self.dad.as_mut(),
                self.stage.as_ref(),
            );
        }

        let player_holding_sustain = self.session.is_player_holding_sustain();
        let is_holding_player = player_holding_sustain || self.session.is_player_holding_key();
        if player_holding_sustain {
            if let Some(bf) = &mut self.bf {
                bf.reset_hold_timer();
            }
        }
        if self.session.is_opponent_holding_sustain() {
            if let Some(dad) = &mut self.dad {
                dad.reset_hold_timer();
            }
        }

        {
            let cur_step = self.session.conductor.current_step;
            let cur_beat = self.session.conductor.current_beat;

            if gameplay.conductor.step_hit {
                self.camera_rig.on_step_hit(cur_step);
                if let Some(st) = &mut self.stage {
                    st.on_step_hit(cur_step);
                }
                if let Some(bf) = &mut self.bf {
                    bf.on_step_hit(cur_step, is_holding_player);
                }
                if let Some(dad) = &mut self.dad {
                    dad.on_step_hit(cur_step, false);
                }
                if let Some(gf) = &mut self.gf {
                    gf.on_step_hit(cur_step, false);
                }
                self.icon_p1.on_step_hit(cur_step);
                self.icon_p2.on_step_hit(cur_step);
            }

            if gameplay.conductor.beat_hit {
                let cues = self.scripts.on_beat_hit(
                    cur_beat,
                    self.stage.as_mut(),
                    self.bf.as_mut(),
                    self.dad.as_mut(),
                    self.gf.as_mut(),
                );
                if let Some(ref mut engine) = audio {
                    for cue in cues.cues.iter().take(cues.count).flatten() {
                        let _ = engine.send(AudioCommand::PlaySfx {
                            slot: cue.slot,
                            gain: cue.gain,
                        });
                    }
                }
                if let Some(st) = &mut self.stage {
                    st.on_beat_hit(cur_beat);
                }
                if let Some(bf) = &mut self.bf {
                    bf.on_beat_hit(cur_beat, is_holding_player);
                }
                if let Some(dad) = &mut self.dad {
                    dad.on_beat_hit(cur_beat, false);
                }
                if !self.scripts.owns_gf_dance() {
                    if let Some(gf) = &mut self.gf {
                        gf.on_beat_hit(cur_beat, false);
                    }
                }
            }
        }

        let beat_len_ms = self.session.conductor.beat_length_ms();
        let update_cues = self.scripts.update(
            elapsed_s,
            song_pos,
            beat_len_ms,
            self.session.health,
            self.stage.as_mut(),
            self.bf.as_mut(),
            self.dad.as_mut(),
            self.gf.as_mut(),
        );
        if let Some(ref mut engine) = audio {
            for cue in update_cues.cues.iter().take(update_cues.count).flatten() {
                let _ = engine.send(AudioCommand::PlaySfx {
                    slot: cue.slot,
                    gain: cue.gain,
                });
            }
        }
        if let Some(st) = &mut self.stage {
            st.update(elapsed_s);
        }
        if let Some(bf) = &mut self.bf {
            bf.update(elapsed_s, beat_len_ms, is_holding_player);
        }
        if let Some(dad) = &mut self.dad {
            dad.update(elapsed_s, beat_len_ms, false);
        }
        if let Some(gf) = &mut self.gf {
            gf.update(elapsed_s, beat_len_ms, false);
        }
        self.camera_rig.update(elapsed_s);

        let bar_layout = HealthBarLayout::compute(
            self.health_bar_size[0],
            self.health_bar_size[1],
            self.session.config.is_downscroll,
            self.session.health_lerp,
        );
        self.icon_p1
            .update(elapsed_s, self.session.health_lerp, &bar_layout);
        self.icon_p2
            .update(elapsed_s, self.session.health_lerp, &bar_layout);

        if !self.countdown.active && !self.session.is_player_dying && !self.results.is_active {
            if let Some(end_ms) = self.scene_config.song_duration_ms {
                if song_pos >= end_ms {
                    self.results.show(
                        &self.scene_config.song_name,
                        &self.scene_config.difficulty,
                        self.session.score(),
                        self.session.tallies,
                        false,
                    );
                    out.song_finished_now = true;
                }
            }
        }

        out
    }

    /// Queues world (`stage_sprites`), HUD (`bg_sprites`, `mesh_batcher`, `fg_sprites`),
    /// and overlay (`overlay_sprites`) geometry with **zero heap allocations**.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        stage_sprites: &mut SpriteBatcher,
        bg_sprites: &mut SpriteBatcher,
        fg_sprites: &mut SpriteBatcher,
        mesh_batcher: &mut MeshBatcher,
        overlay_sprites: &mut SpriteBatcher,
        style: &StrumlineVisualStyle,
        note_assets: Option<&NoteStyleAssets>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        let cam_pos = [
            self.camera_rig.cam_game.scroll[0] - self.camera_rig.cam_game.shake_offset[0],
            self.camera_rig.cam_game.scroll[1] - self.camera_rig.cam_game.shake_offset[1],
        ];
        let cam_zoom = self.camera_rig.cam_game.effective_zoom();
        let hud_zoom = self.camera_rig.cam_hud.effective_zoom();

        if self.game_over.phase != GameOverPhase::Inactive {
            if let Some(bf) = &self.bf {
                bf.draw(stage_sprites, cam_pos, cam_zoom);
            }
            self.game_over
                .draw(overlay_sprites, style.white_texture, font, font_tex);
            return;
        }

        if self.results.is_active {
            self.results
                .draw(overlay_sprites, style.white_texture, font, font_tex);
            return;
        }

        self.draw_world_layer(stage_sprites, cam_pos, cam_zoom, hud_zoom);

        self.session.draw(
            bg_sprites,
            fg_sprites,
            mesh_batcher,
            style,
            note_assets,
            font,
            font_tex,
        );
        self.icon_p2.draw(fg_sprites, style.white_texture);
        self.icon_p1.draw(fg_sprites, style.white_texture);
        self.countdown
            .draw(fg_sprites, self.countdown_assets.as_ref());

        if let Some(flash) = self.camera_rig.cam_game.flash_overlay() {
            overlay_sprites.push(
                style.white_texture,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color(flash),
            );
        }
        if let Some(flash) = self.camera_rig.cam_hud.flash_overlay() {
            overlay_sprites.push(
                style.white_texture,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color(flash),
            );
        }

        self.pause_menu
            .draw(overlay_sprites, style.white_texture, font, font_tex);
    }

    fn draw_world_layer(
        &self,
        stage_sprites: &mut SpriteBatcher,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        hud_zoom: f32,
    ) {
        match (self.stage.as_ref(), self.gf.as_ref(), self.dad.as_ref(), self.bf.as_ref()) {
            (Some(st), Some(g), Some(d), Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g, d, b],
                    &self.scripts,
                    Some(d),
                    Some(g),
                );
            }
            (Some(st), Some(g), Some(d), None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g, d],
                    &self.scripts,
                    Some(d),
                    Some(g),
                );
            }
            (Some(st), Some(g), None, Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g, b],
                    &self.scripts,
                    None,
                    Some(g),
                );
            }
            (Some(st), None, Some(d), Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[d, b],
                    &self.scripts,
                    Some(d),
                    None,
                );
            }
            (Some(st), Some(g), None, None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g],
                    &self.scripts,
                    None,
                    Some(g),
                );
            }
            (Some(st), None, Some(d), None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[d],
                    &self.scripts,
                    Some(d),
                    None,
                );
            }
            (Some(st), None, None, Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[b],
                    &self.scripts,
                    None,
                    None,
                );
            }
            (Some(st), None, None, None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[],
                    &self.scripts,
                    None,
                    None,
                );
            }
            (None, g, d, b) => {
                draw_optional_chars_sorted(stage_sprites, cam_pos, cam_zoom, [g, d, b]);
            }
        }
    }
}

fn draw_optional_chars_sorted(
    stage_sprites: &mut SpriteBatcher,
    cam_pos: [f32; 2],
    cam_zoom: f32,
    chars: [Option<&BaseCharacter>; 3],
) {
    let mut entries: [(i32, usize); 3] = [(i32::MAX, 0), (i32::MAX, 1), (i32::MAX, 2)];
    let mut count = 0usize;
    for (idx, ch_opt) in chars.iter().enumerate() {
        if let Some(ch) = ch_opt {
            entries[count] = (ch.z_index, idx);
            count += 1;
        }
    }
    for i in 1..count {
        let key = entries[i];
        let mut j = i;
        while j > 0 && entries[j - 1].0 > key.0 {
            entries[j] = entries[j - 1];
            j -= 1;
        }
        entries[j] = key;
    }
    for &(_, idx) in entries.iter().take(count) {
        if let Some(ch) = chars[idx] {
            ch.draw(stage_sprites, cam_pos, cam_zoom);
        }
    }
}

fn initial_camera_focus(
    bf: Option<&BaseCharacter>,
    dad: Option<&BaseCharacter>,
    gf: Option<&BaseCharacter>,
) -> [f32; 2] {
    dad.or(gf)
        .or(bf)
        .map_or([VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5], BaseCharacter::get_camera_focus_point)
}

#[allow(clippy::too_many_arguments)]
fn apply_song_event(
    ev: &QueuedSongEvent,
    step_len_ms: f64,
    base_scroll_speed: f64,
    camera_rig: &mut CameraRig,
    bf: Option<&mut BaseCharacter>,
    dad: Option<&mut BaseCharacter>,
    gf: Option<&mut BaseCharacter>,
    stage: Option<&mut Stage>,
    player_strumline: &mut Strumline,
    opponent_strumline: &mut Strumline,
    player_tween: &mut ScrollSpeedTween,
    opponent_tween: &mut ScrollSpeedTween,
) {
    match &ev.action {
        GameEventAction::FocusCamera {
            target,
            offset,
            duration_steps,
            ease,
        } => {
            let base = match target {
                CameraFocusTarget::Position => [0.0, 0.0],
                CameraFocusTarget::Player => bf.map_or(
                    [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5],
                    |c| c.get_camera_focus_point(),
                ),
                CameraFocusTarget::Opponent => dad.map_or(
                    [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5],
                    |c| c.get_camera_focus_point(),
                ),
                CameraFocusTarget::Girlfriend => gf.map_or(
                    [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5],
                    |c| c.get_camera_focus_point(),
                ),
                CameraFocusTarget::Unknown(_) => camera_rig.cam_game.target_follow,
            };
            let tx = base[0] + offset[0];
            let ty = base[1] + offset[1];
            match *ease {
                FocusCameraEase::Classic => camera_rig.cam_game.set_target(tx, ty),
                FocusCameraEase::Tween(flx) => camera_rig.cam_game.tween_to_steps(
                    tx,
                    ty,
                    *duration_steps,
                    step_len_ms,
                    flx,
                ),
            }
        }
        GameEventAction::ZoomCamera {
            zoom,
            duration_steps,
            mode,
            ease,
        } => {
            let direct = matches!(mode, ZoomMode::Direct);
            camera_rig.cam_game.tween_zoom_steps(
                *zoom,
                direct,
                *duration_steps,
                step_len_ms,
                *ease,
            );
        }
        GameEventAction::SetCameraBop {
            rate_beats,
            offset_beats,
            intensity,
        } => {
            camera_rig.set_bop(*rate_beats, *offset_beats, *intensity);
        }
        GameEventAction::PlayAnimation {
            target,
            anim,
            force,
        } => match GameEventAction::resolve_animation_target(target) {
            AnimationTarget::Boyfriend => {
                if let Some(c) = bf {
                    c.play_animation(anim, *force, false);
                }
            }
            AnimationTarget::Dad => {
                if let Some(c) = dad {
                    c.play_animation(anim, *force, false);
                }
            }
            AnimationTarget::Girlfriend => {
                if let Some(c) = gf {
                    c.play_animation(anim, *force, false);
                }
            }
            AnimationTarget::Prop(prop_name) => {
                if let Some(st) = stage {
                    if let Some(prop) = st.get_named_prop_mut(prop_name) {
                        if let Some(bop) = &mut prop.bopper {
                            bop.play_animation(anim, *force);
                        }
                    }
                }
            }
        },
        GameEventAction::ScrollSpeed {
            scroll,
            duration_steps,
            ease,
            strumline,
            absolute,
        } => {
            let target = if *absolute {
                f64::from(*scroll)
            } else {
                base_scroll_speed * f64::from(*scroll)
            }
            .max(0.1);
            let duration_s = ((f64::from(*duration_steps) * step_len_ms) / MS_PER_SEC) as f32;

            if matches!(
                strumline,
                ScrollSpeedTarget::Both | ScrollSpeedTarget::Player
            ) {
                if let Some(immediate) =
                    player_tween.start(player_strumline.scroll_speed, target, duration_s, *ease)
                {
                    player_strumline.scroll_speed = immediate;
                }
            }
            if matches!(
                strumline,
                ScrollSpeedTarget::Both | ScrollSpeedTarget::Opponent
            ) {
                if let Some(immediate) = opponent_tween.start(
                    opponent_strumline.scroll_speed,
                    target,
                    duration_s,
                    *ease,
                ) {
                    opponent_strumline.scroll_speed = immediate;
                }
            }
        }
        GameEventAction::Custom { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fnfrs_core::constants::HIT_WINDOW_MS;
    use fnfrs_core::schema::character::{AnimationData, CharacterData, CharacterRenderType};
    use fnfrs_core::schema::song::SongEventData;
    use fnfrs_core::schema::stage::{ScaleSpec, StageData, StageDataCharacters, StageDataProp};
    use winit::keyboard::KeyCode;

    use super::*;
    use crate::character::CharacterKind;

    fn make_anim(name: &str, looped: bool) -> AnimationData {
        AnimationData {
            name: name.to_owned(),
            prefix: Some(name.to_owned()),
            asset_path: None,
            offsets: [0.0, 0.0],
            looped,
            flip_x: false,
            flip_y: false,
            frame_rate: 24,
            frame_indices: Vec::new(),
            anim_type: "framelabel".to_owned(),
            render_type: String::new(),
            atlas_settings: None,
        }
    }

    fn make_test_character(id: &str, kind: CharacterKind) -> BaseCharacter {
        let data = CharacterData {
            version: "1.0.2".to_owned(),
            name: id.to_owned(),
            render_type: CharacterRenderType::Sparrow,
            asset_path: format!("characters/{id}"),
            scale: 1.0,
            health_icon: None,
            death: None,
            offsets: [0.0, 0.0],
            camera_offsets: [0.0, 0.0],
            is_pixel: false,
            dance_every: 1.0,
            sing_time: 4.0,
            animations: vec![
                make_anim("idle", false),
                make_anim("singLEFT", false),
                make_anim("singDOWN", false),
                make_anim("singUP", false),
                make_anim("singRIGHT", false),
                make_anim("singLEFTmiss", false),
                make_anim("hey", false),
            ],
            starting_animation: "idle".to_owned(),
            flip_x: false,
            apply_stage_matrix: false,
            atlas_settings: None,
        };
        BaseCharacter::new_synthetic(id, kind, [200.0, 300.0], data)
    }

    #[test]
    fn play_session_hits_and_misses_within_pbot1_window() {
        let notes = vec![
            SongNoteData {
                time: 1000.0,
                data: 0, // Left
                length: 0.0,
                kind: None,
                params: Vec::new(),
            },
            SongNoteData {
                time: 2000.0,
                data: 3, // Right
                length: 0.0,
                kind: None,
                params: Vec::new(),
            },
        ];

        let mut session = PlaySession::from_raw_notes(120.0, &notes, &[], PlayConfig::default());
        let style = StrumlineVisualStyle::fallback(TextureHandle(0));
        let mut inputs = InputQueue::default();
        let t0 = Instant::now();

        // Advance near 1005ms (5ms late -> Sick, ~500 pts).
        session.update(900.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        inputs.record_key(KeyCode::KeyA, true, false, t0);
        let ev = session.update(1004.0, 1.0 / 60.0, t0, &mut inputs, None, &style);

        assert_eq!(ev.last_judgement, Some(Judgement::Sick));
        assert_eq!(session.tallies.sick, 1);
        assert_eq!(session.tallies.combo, 1);
        assert!(session.score() >= 490);

        // Advance past 2000 + HIT_WINDOW_MS without hitting second note -> miss.
        let later = t0 + Duration::from_millis(1200);
        let ev_miss = session.update(
            2000.0 + HIT_WINDOW_MS + 10.0,
            1.0 / 60.0,
            later,
            &mut inputs,
            None,
            &style,
        );
        assert!(ev_miss.missed_note);
        assert_eq!(session.tallies.missed, 1);
        assert_eq!(session.tallies.combo, 0);
    }

    #[test]
    fn play_scene_drives_countdown_events_camera_chars_and_icons() {
        let player_notes = vec![SongNoteData {
            time: 1000.0,
            data: 0,
            length: 0.0,
            kind: None,
            params: Vec::new(),
        }];
        let opponent_notes = vec![SongNoteData {
            time: 500.0,
            data: 6, // Up on opponent strumline
            length: 0.0,
            kind: None,
            params: Vec::new(),
        }];

        let play_cfg = PlayConfig {
            scroll_speed: 1.5,
            ..Default::default()
        };
        let session =
            PlaySession::from_raw_notes(120.0, &player_notes, &opponent_notes, play_cfg);

        let mut stage_chars = StageDataCharacters::default();
        stage_chars.bf.position = [950.0, 800.0];
        stage_chars.bf.camera_offsets = [-100.0, -100.0];
        stage_chars.dad.position = [250.0, 800.0];
        stage_chars.dad.camera_offsets = [100.0, -100.0];

        let stage_data = StageData {
            version: "1.0.0".to_owned(),
            name: "Test".to_owned(),
            props: vec![StageDataProp {
                name: Some("bg".to_owned()),
                asset_path: "#112233".to_owned(),
                position: [0.0, 0.0],
                z_index: 10,
                is_pixel: false,
                flip_x: false,
                flip_y: false,
                scale: ScaleSpec::Pair([1280.0, 720.0]),
                alpha: 1.0,
                dance_every: 0.0,
                scroll: [0.0, 0.0],
                animations: Vec::new(),
                starting_animation: None,
                anim_type: "sparrow".to_owned(),
                angle: 0.0,
                blend: String::new(),
                color: "#FFFFFF".to_owned(),
                atlas_settings: None,
            }],
            characters: stage_chars,
            camera_zoom: 0.9,
            directory: "shared".to_owned(),
        };
        let stage = Stage::from_data_headless("test", stage_data);
        let bf = make_test_character("bf", CharacterKind::Bf);
        let dad = make_test_character("dad", CharacterKind::Dad);
        let gf = make_test_character("gf", CharacterKind::Gf);

        let raw_events: Vec<SongEventData> = serde_json::from_str(
            r#"[
                {"t": 0.0, "e": "FocusCamera", "v": {"char": 0, "ease": "INSTANT"}},
                {"t": 0.0, "e": "ZoomCamera", "v": {"zoom": 1.2, "duration": 0, "mode": "direct", "ease": "INSTANT"}},
                {"t": 250.0, "e": "PlayAnimation", "v": {"target": "bf", "anim": "hey", "force": true}},
                {"t": 250.0, "e": "ScrollSpeed", "v": {"scroll": 2.0, "duration": 0, "ease": "INSTANT", "absolute": true}}
            ]"#,
        )
        .expect("valid event json");
        let events = SongEventQueue::from_raw_events(&raw_events);

        let mut scene = PlayScene::new(
            session,
            Some(stage),
            Some(bf),
            Some(dad),
            Some(gf),
            events,
            HealthIcon::placeholder("bf", true, false),
            HealthIcon::placeholder("dad", false, false),
            Some(CountdownAssets::empty(false)),
            PlaySceneConfig {
                song_name: "Test Song".to_owned(),
                difficulty: "hard".to_owned(),
                start_countdown: true,
                song_duration_ms: Some(2500.0),
                death_count: 0,
            },
        );

        let style = StrumlineVisualStyle::fallback(TextureHandle(0));
        let mut inputs = InputQueue::default();
        let t0 = Instant::now();

        // 1. Pre-roll countdown (-4 * 500ms = -2000ms -> 0ms at 120 BPM).
        let c0 = scene.update(0.0, 0.0, t0, &mut inputs, None, &style);
        assert_eq!(c0.countdown_step, Some(CountdownStep::Three));
        assert_eq!(c0.countdown_sfx, Some(CountdownSfx::Intro3));
        assert!(scene.icon_p1.bop_scale_mult > 1.0);

        let c1 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert_eq!(c1.countdown_step, Some(CountdownStep::Two));
        assert_eq!(c1.countdown_sfx, Some(CountdownSfx::Intro2));

        let c2 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert_eq!(c2.countdown_step, Some(CountdownStep::One));
        assert_eq!(c2.countdown_sfx, Some(CountdownSfx::Intro1));

        let c3 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert_eq!(c3.countdown_step, Some(CountdownStep::Go));
        assert_eq!(c3.countdown_sfx, Some(CountdownSfx::IntroGo));

        let c4 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert!(c4.countdown_finished_now);
        assert!(!scene.countdown.active);

        // FocusCamera (BF) + ZoomCamera (1.2 direct) fired at t=0.
        let bf_focus = scene.bf.as_ref().unwrap().get_camera_focus_point();
        assert!((scene.camera_rig.cam_game.follow_point[0] - bf_focus[0]).abs() < 1e-3);
        assert!((scene.camera_rig.cam_game.zoom - 1.2).abs() < 1e-4);

        // 2. Advance to 250ms -> PlayAnimation("hey") on BF + ScrollSpeed(2.0).
        let _ = scene.update(250.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert_eq!(scene.bf.as_ref().unwrap().current_animation(), "hey");
        assert!((scene.session.player_strumline.scroll_speed - 2.0).abs() < 1e-6);
        assert!((scene.session.opponent_strumline.scroll_speed - 2.0).abs() < 1e-6);

        // 3. Advance to 500ms -> Opponent hits Up note and sings singUP.
        let opp_ev = scene.update(500.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert_eq!(opp_ev.gameplay.opponent_hit_dir, Some(NoteDirection::Up));
        assert_eq!(scene.dad.as_ref().unwrap().current_animation(), "singUP");

        // 4. Hit player Left note at 1000ms -> BF sings singLEFT.
        inputs.record_key(KeyCode::KeyA, true, false, t0);
        let p_ev = scene.update(1000.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert_eq!(p_ev.gameplay.player_hit_dir, Some(NoteDirection::Left));
        assert_eq!(scene.bf.as_ref().unwrap().current_animation(), "singLEFT");

        // 5. Reach song_duration_ms (2500ms) -> ResultsSummaryState activates.
        let end_ev = scene.update(2500.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert!(end_ev.song_finished_now);
        assert!(scene.results.is_active);
    }
}
