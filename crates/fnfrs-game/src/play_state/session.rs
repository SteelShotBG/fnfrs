use std::time::Instant;

use fnfrs_audio::{AudioCommand, AudioEngine, TrackKind};
use fnfrs_core::conductor::{Conductor, ConductorEvents};
use fnfrs_core::constants::{
    HEALTH_HOLD_BONUS_PER_SECOND, HEALTH_MAX, HEALTH_MIN, HEALTH_MISS_PENALTY, HEALTH_STARTING,
    SCORE_GHOST_MISS_PENALTY, SCORE_HOLD_BONUS_PER_SECOND, STRUMLINE_SIZE, STRUMLINE_X_OFFSET,
    STRUMLINE_Y_OFFSET, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::pool::PoolHandle;
use fnfrs_core::schema::song::{SongChartData, SongMetadata, SongNoteData};
use fnfrs_core::scoring::{
    format_score_into, get_miss_score, judge_note, score_note, Judgement, ScoreTallies,
    ScoringSystem,
};
use fnfrs_render::{BitmapFont, MeshBatcher, SpriteBatcher, TextAlign, TextStyle, TextureHandle};

use crate::input::{compute_note_diff_ms, InputQueue, NoteDirection};
use crate::note_style::NoteStyleAssets;
use crate::popup::PopUpPool;
use crate::scripts::NoteKindTag;
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
    pub combo_hit_count: Option<u32>,
    pub combo_broken_from: Option<u32>,
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
            !config.is_botplay,
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
            !config.is_botplay,
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

    /// Toggles Bot Play mode and synchronizes `player_strumline.is_player` (`!is_botplay`).
    pub fn set_botplay(&mut self, is_botplay: bool) {
        self.config.is_botplay = is_botplay;
        self.player_strumline.is_player = !is_botplay;
        if is_botplay {
            self.player_strumline.clear_held_keys();
        }
        self.refresh_hud_buffers();
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

        // Botplay hits on player strumline (`PlayState.processNotes` + `Strumline.hitNote`).
        let mut bot_hit_any = false;
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
                bot_hit_any = true;
            }
        }
        if bot_hit_any {
            if let Some(engine) = audio.as_deref_mut() {
                let _ = engine.send(AudioCommand::SetTrackGain {
                    track: TrackKind::PlayerVocals,
                    gain: self.config.player_vocals_gain,
                });
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
                let prev_combo = self.tallies.combo;
                self.on_note_miss(audio.as_deref_mut());
                step_events.missed_note = true;
                if prev_combo > 0 {
                    step_events.combo_broken_from = Some(
                        step_events.combo_broken_from.unwrap_or(0).max(prev_combo),
                    );
                }
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
                        let prev_combo = self.tallies.combo;
                        self.apply_score(
                            penalty.score_change,
                            None,
                            penalty.health_change,
                            true,
                        );
                        if prev_combo > 0 {
                            step_events.combo_broken_from = Some(
                                step_events.combo_broken_from.unwrap_or(0).max(prev_combo),
                            );
                        }
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
        if self.config.is_botplay {
            inputs.clear();
            return;
        }
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
            let prev_combo = self.tallies.combo;
            self.apply_score(points, Some(judgement), health_change, breaks_combo);
            if breaks_combo {
                if prev_combo > 0 {
                    step_events.combo_broken_from = Some(
                        step_events.combo_broken_from.unwrap_or(0).max(prev_combo),
                    );
                }
            } else {
                step_events.combo_hit_count = Some(self.tallies.combo);
            }
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

        // Judgement & combo popups drawn strictly in chronological spawn order so
        // newer popups never render behind older still-alive popups (`PopUpStuff.hx`).
        if !self.hide_popups {
            let mut digit_byte = [b'0'; 1];
            for item in self.popups.iter_ordered() {
                match item {
                    crate::popup::PopUpOrderedItem::Rating(rating) => {
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
                    crate::popup::PopUpOrderedItem::Digit(digit) => {
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
            letter_spacing: -1.0,
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
