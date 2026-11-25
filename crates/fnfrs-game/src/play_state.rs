//! Core V-Slice gameplay session state (`PlayState.hx`) coordinating
//! `Conductor`, dual `Strumline`s, `InputQueue`, `PopUpPool`, `ScoreTallies`,
//! health mechanics, and `AudioEngine` SFX/stem controls with zero per-frame
//! heap allocations.

use std::fmt::Write as _;
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
use fnfrs_render::{BitmapFont, MeshBatcher, SpriteBatcher, StackTextBuffer, TextAlign, TextStyle, TextureHandle};

use crate::input::{compute_note_diff_ms, InputQueue, NoteDirection};
use crate::popup::PopUpPool;
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
            ghost_tapping: true,
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
    pub died: bool,
    pub player_hit_dir: Option<NoteDirection>,
    pub opponent_hit_dir: Option<NoteDirection>,
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

    score_text_buf: [u8; 32],
    stats_buf: StackTextBuffer<96>,
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
            score_text_buf: [0u8; 32],
            stats_buf: StackTextBuffer::new(),
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
            score_text_buf: [0u8; 32],
            stats_buf: StackTextBuffer::new(),
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
            if let Some(note) = self.player_strumline.notes.get_mut(*miss_h) {
                note.handled_miss = true;
            }
            if !self.config.is_botplay {
                self.on_note_miss(audio.as_deref_mut());
                step_events.missed_note = true;
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
            if let Some(engine) = audio.as_deref_mut() {
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
        let Some((note_time_ms, direction, scoreable, hold_handle)) = self
            .player_strumline
            .notes
            .get(note_handle)
            .map(|n| (n.strum_time_ms, n.direction, n.scoreable, n.hold_handle))
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
            self.popups.spawn_rating(
                judgement,
                [260.0, 96.0],
                [0.0, 0.0],
                self.conductor.beat_length_ms(),
                style.is_pixel,
            );
            self.popups.spawn_combo(
                self.tallies.combo,
                [0.0, 0.0],
                self.conductor.beat_length_ms(),
                style.is_pixel,
            );
        }

        step_events.last_judgement = Some(judgement);
        step_events.last_note_diff_ms = Some(truncated_diff);
        step_events.player_hit_dir = Some(direction);
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

    fn refresh_hud_buffers(&mut self) {
        let score = self.score();
        let _ = format_score_into(&mut self.score_text_buf, score, self.config.is_botplay);

        self.stats_buf.clear();
        let _ = write!(
            self.stats_buf,
            "Combo: {} | Misses: {} | Sick: {}",
            self.tallies.combo, self.tallies.missed, self.tallies.sick
        );
    }

    /// Zero-allocation formatted HUD score bytes (`"Score: 1,234,560"` or `"Bot Play Enabled"`).
    #[must_use]
    pub fn score_text_bytes(&self) -> &[u8] {
        let len = self
            .score_text_buf
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.score_text_buf.len());
        &self.score_text_buf[..len]
    }

    /// Queues strumlines, popups, health bar, and HUD text into `sprite_batcher` &
    /// `mesh_batcher` with zero heap allocations.
    pub fn draw(
        &self,
        sprite_batcher: &mut SpriteBatcher,
        mesh_batcher: &mut MeshBatcher,
        style: &StrumlineVisualStyle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        let song_pos = self.conductor.song_position_ms;
        self.opponent_strumline
            .draw(sprite_batcher, mesh_batcher, style, song_pos);
        self.player_strumline
            .draw(sprite_batcher, mesh_batcher, style, song_pos);

        // Judgement & combo popups.
        for (_, rating) in self.popups.ratings.iter_alive() {
            let (label, color): (&str, [f32; 4]) = match rating.judgement {
                Judgement::Sick => ("SICK!", [0.22, 0.94, 1.0, rating.alpha]),
                Judgement::Good => ("GOOD!", [0.35, 0.95, 0.45, rating.alpha]),
                Judgement::Bad => ("BAD", [1.0, 0.72, 0.25, rating.alpha]),
                Judgement::Shit => ("SHIT", [0.85, 0.30, 0.30, rating.alpha]),
                Judgement::Miss => ("MISS", [0.75, 0.15, 0.20, rating.alpha]),
            };
            font.draw_str(
                sprite_batcher,
                font_tex,
                label,
                rating.x + 80.0,
                rating.y + 30.0,
                &TextStyle {
                    scale: 4.0,
                    color,
                    ..Default::default()
                },
            );
        }

        let mut digit_byte = [b'0'; 1];
        for (_, digit) in self.popups.digits.iter_alive() {
            digit_byte[0] = b'0' + (digit.digit % 10);
            font.draw_bytes(
                sprite_batcher,
                font_tex,
                &digit_byte,
                digit.x,
                digit.y,
                &TextStyle {
                    scale: 3.5,
                    color: [1.0, 1.0, 1.0, digit.alpha],
                    ..Default::default()
                },
            );
        }

        // Minimal V-Slice health bar + HUD score line.
        let bar_w = 593.0;
        let bar_h = 11.0;
        let bar_x = (VIRTUAL_WIDTH - bar_w) * 0.5;
        let bar_y = if self.config.is_downscroll {
            VIRTUAL_HEIGHT * 0.10
        } else {
            VIRTUAL_HEIGHT * 0.89
        };
        let player_ratio = (self.health_lerp / HEALTH_MAX).clamp(0.0, 1.0);
        let split_x = bar_w * (1.0 - player_ratio);

        sprite_batcher.push(
            style.white_texture,
            fnfrs_render::BlendMode::Normal,
            fnfrs_render::FilterMode::Nearest,
            fnfrs_render::SpriteInstance::new([bar_x - 4.0, bar_y - 4.0], [bar_w + 8.0, bar_h + 8.0])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );
        // Opponent left fill (purple), player right fill (cyan/green).
        sprite_batcher.push(
            style.white_texture,
            fnfrs_render::BlendMode::Normal,
            fnfrs_render::FilterMode::Nearest,
            fnfrs_render::SpriteInstance::new([bar_x, bar_y], [split_x, bar_h])
                .with_color([0.69, 0.40, 0.81, 1.0]),
        );
        sprite_batcher.push(
            style.white_texture,
            fnfrs_render::BlendMode::Normal,
            fnfrs_render::FilterMode::Nearest,
            fnfrs_render::SpriteInstance::new([bar_x + split_x, bar_y], [bar_w - split_x, bar_h])
                .with_color([0.19, 0.69, 0.82, 1.0]),
        );

        let hud_style = TextStyle {
            scale: 2.0,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Center,
            ..Default::default()
        };
        font.draw_bytes(
            sprite_batcher,
            font_tex,
            self.score_text_bytes(),
            VIRTUAL_WIDTH * 0.5 + 210.0,
            bar_y + 22.0,
            &hud_style,
        );
        font.draw_bytes(
            sprite_batcher,
            font_tex,
            self.stats_buf.as_bytes(),
            VIRTUAL_WIDTH * 0.5 - 150.0,
            bar_y + 22.0,
            &hud_style,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fnfrs_core::constants::HIT_WINDOW_MS;
    use winit::keyboard::KeyCode;

    use super::*;

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
}
