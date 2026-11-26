//! Zero-allocation V-Slice strumline manager, note/splash pools, and
//! `GRhythmUtil` window processing (`Strumline.hx`, `StrumlineNote.hx`,
//! `NoteSprite.hx`, `NoteSplash.hx`).

use fnfrs_core::constants::{
    GHOST_TAP_DELAY, HIT_WINDOW_MS, KEY_COUNT, NOTE_SPACING, NOTE_SPLASH_CAP, PIXELS_PER_MS,
    STRUMLINE_SIZE, VIRTUAL_HEIGHT,
};
use fnfrs_core::pool::{FixedPool, PoolHandle};
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_render::{
    BlendMode, FilterMode, MeshBatcher, SpriteBatcher, SpriteInstance, TextureHandle,
};
use winit::keyboard::KeyCode;

use crate::input::{HeldKeys, NoteDirection};
use crate::note_style::{push_sparrow_frame, NoteStyleAssets, SparrowDrawParams};
use crate::scripts::NoteKindTag;
use crate::sustain::{advance_hold_covers, HoldCoverPhase, NoteHoldCoverState, SustainTrailState};

/// Default pre-allocated pool size per strumline for visible note heads.
pub const NOTE_POOL_CAPACITY: usize = 256;

/// Default pre-allocated pool size per strumline for active sustain trails.
pub const HOLD_POOL_CAPACITY: usize = 128;

/// Default pre-allocated pool size per strumline for sustain hold covers.
pub const HOLD_COVER_POOL_CAPACITY: usize = 16;

/// Duration opponent receptors hold the confirm pose before returning to static
/// (`StrumlineNote.CONFIRM_HOLD_TIME`).
pub const CONFIRM_HOLD_TIME_S: f32 = 0.15;

/// Default Sparrow frame rate for receptors/splashes.
pub const NOTE_ANIM_FPS: f32 = 24.0;

/// Animation state of a single strumline receptor (`StrumlineNote.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReceptorAnim {
    #[default]
    Static,
    Press,
    Confirm,
    ConfirmHold,
}

/// One of the 4 receptors on a [`Strumline`].
#[derive(Debug, Clone)]
pub struct ReceptorState {
    pub direction: NoteDirection,
    pub anim: ReceptorAnim,
    pub anim_time_s: f32,
    pub anim_frame: usize,
    pub confirm_hold_timer_s: Option<f32>,
    pub x: f32,
    pub y: f32,
    pub alpha: f32,
}

impl ReceptorState {
    #[must_use]
    pub const fn new(direction: NoteDirection, x: f32, y: f32) -> Self {
        Self {
            direction,
            anim: ReceptorAnim::Static,
            anim_time_s: 0.0,
            anim_frame: 0,
            confirm_hold_timer_s: None,
            x,
            y,
            alpha: 1.0,
        }
    }

    pub fn play_static(&mut self) {
        self.anim = ReceptorAnim::Static;
        self.anim_time_s = 0.0;
        self.anim_frame = 0;
        self.confirm_hold_timer_s = None;
    }

    pub fn play_press(&mut self) {
        self.anim = ReceptorAnim::Press;
        self.anim_time_s = 0.0;
        self.anim_frame = 0;
        self.confirm_hold_timer_s = None;
    }

    pub fn play_confirm(&mut self, is_player: bool) {
        self.anim = ReceptorAnim::Confirm;
        self.anim_time_s = 0.0;
        self.anim_frame = 0;
        // Opponent resets after CONFIRM_HOLD_TIME_S; player finishes confirm animation first.
        self.confirm_hold_timer_s = if is_player { None } else { Some(0.0) };
    }

    pub fn hold_confirm(&mut self) {
        self.confirm_hold_timer_s = None;
        if self.anim != ReceptorAnim::ConfirmHold {
            self.anim = ReceptorAnim::ConfirmHold;
            self.anim_time_s = 0.0;
            self.anim_frame = 0;
        }
    }

    pub fn update(&mut self, elapsed_s: f32, is_player: bool, confirm_frames: usize) {
        self.anim_time_s += elapsed_s.max(0.0);
        let raw_frame = (self.anim_time_s * NOTE_ANIM_FPS).floor() as usize;

        match self.anim {
            ReceptorAnim::Static | ReceptorAnim::Press => {
                self.anim_frame = raw_frame;
            }
            ReceptorAnim::Confirm => {
                let count = confirm_frames.max(1);
                if raw_frame >= count {
                    self.anim_frame = count - 1;
                    if is_player && self.confirm_hold_timer_s.is_none() {
                        self.confirm_hold_timer_s = Some(0.0);
                    }
                } else {
                    self.anim_frame = raw_frame;
                }
            }
            ReceptorAnim::ConfirmHold => {
                self.anim_frame = raw_frame;
            }
        }

        if let Some(timer) = &mut self.confirm_hold_timer_s {
            *timer += elapsed_s.max(0.0);
            if *timer >= CONFIRM_HOLD_TIME_S {
                self.play_static();
            }
        }
    }
}

/// Pooled note head (`NoteSprite.hx`).
#[derive(Debug, Clone)]
pub struct NoteEntity {
    pub strum_time_ms: f64,
    pub sustain_length_ms: f64,
    pub direction: NoteDirection,
    pub kind: NoteKindTag,
    pub scoreable: bool,
    pub low_priority: bool,
    pub has_been_hit: bool,
    pub too_early: bool,
    pub may_hit: bool,
    pub has_missed: bool,
    pub handled_miss: bool,
    pub visible: bool,
    pub alpha: f32,
    pub desaturated: bool,
    pub x: f32,
    pub y: f32,
    pub y_offset: f32,
    pub hold_handle: Option<PoolHandle>,
}

impl Default for NoteEntity {
    fn default() -> Self {
        Self {
            strum_time_ms: 0.0,
            sustain_length_ms: 0.0,
            direction: NoteDirection::Left,
            kind: NoteKindTag::Normal,
            scoreable: true,
            low_priority: false,
            has_been_hit: false,
            too_early: false,
            may_hit: false,
            has_missed: false,
            handled_miss: false,
            visible: true,
            alpha: 1.0,
            desaturated: false,
            x: 0.0,
            y: -9999.0,
            y_offset: 0.0,
            hold_handle: None,
        }
    }
}

/// Pooled note hit splash (`NoteSplash.hx`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoteSplashEntity {
    pub direction: NoteDirection,
    pub variant: usize,
    pub anim_time_s: f32,
    pub frame_index: usize,
    pub x: f32,
    pub y: f32,
    pub visible: bool,
}

/// Result of evaluating a single note against `HIT_WINDOW_MS` (`GRhythmUtil.processWindow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitWindowOutcome {
    pub botplay_hit: bool,
    pub should_continue: bool,
}

/// Visual parameters for drawing a [`Strumline`] into `SpriteBatcher` and
/// `MeshBatcher`. Defaults render crisp geometric/textured fallback arrows so
/// gameplay and tests work before loading `NoteStyle` Sparrow sheets from disk.
#[derive(Debug, Clone, Copy)]
pub struct StrumlineVisualStyle {
    pub white_texture: TextureHandle,
    pub note_texture: Option<TextureHandle>,
    pub hold_texture: Option<TextureHandle>,
    pub hold_tex_size: [f32; 2],
    pub hold_zoom: f32,
    pub hold_offsets: [f32; 2],
    pub is_pixel: bool,
    pub show_splashes: bool,
    pub use_sparrow_offsets: bool,
    pub visual_initial_offset: f32,
    pub note_nudge: f32,
    pub cover_hitbox: [f32; 2],
    pub confirm_frames: usize,
    pub splash_frames: usize,
    pub cover_start_frames: usize,
    pub cover_loop_frames: usize,
    pub cover_end_frames: usize,
}

impl StrumlineVisualStyle {
    /// Creates a self-contained fallback visual style backed by `white_texture`.
    #[must_use]
    pub const fn fallback(white_texture: TextureHandle) -> Self {
        Self {
            white_texture,
            note_texture: None,
            hold_texture: None,
            hold_tex_size: [408.0, 64.0],
            hold_zoom: 0.7,
            hold_offsets: [0.0, 0.0],
            is_pixel: false,
            show_splashes: true,
            use_sparrow_offsets: false,
            visual_initial_offset: 0.0,
            note_nudge: 0.0,
            cover_hitbox: [300.0, 400.0],
            confirm_frames: 4,
            splash_frames: 4,
            cover_start_frames: 3,
            cover_loop_frames: 4,
            cover_end_frames: 3,
        }
    }

    /// Standard V-Slice lane RGB tints (`purple`, `blue`, `green`, `red`).
    #[must_use]
    pub const fn lane_color(dir: NoteDirection) -> [f32; 3] {
        match dir {
            NoteDirection::Left => [0.76, 0.29, 0.60],
            NoteDirection::Down => [0.00, 0.85, 1.00],
            NoteDirection::Up => [0.07, 0.98, 0.02],
            NoteDirection::Right => [0.98, 0.22, 0.25],
        }
    }
}

/// Events discovered while updating a [`Strumline`] for one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct StrumlineFrameEvents {
    pub spawned_count: usize,
    pub newly_missed_handles: [Option<PoolHandle>; 32],
    pub newly_missed_len: usize,
    pub bot_hit_handles: [Option<PoolHandle>; 32],
    pub bot_hit_len: usize,
    pub hold_drop_handles: [Option<PoolHandle>; 16],
    pub hold_drop_len: usize,
    pub active_hold_scoreable_count: usize,
}

/// Fixed-capacity V-Slice strumline owning receptors, notes, sustain trails, hold
/// covers, and note splashes with zero per-frame heap allocations.
#[derive(Debug)]
pub struct Strumline {
    pub is_player: bool,
    pub is_downscroll: bool,
    pub scroll_speed: f64,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub note_spacing_scale: f32,
    pub show_notesplash: bool,
    pub custom_render_distance_ms: Option<f64>,
    pub ghost_tap_timer_s: f64,
    pub strumline_bg_alpha: f32,

    pub receptors: [ReceptorState; KEY_COUNT],
    pub held_keys: HeldKeys,
    pub notes: FixedPool<NoteEntity>,
    pub holds: FixedPool<SustainTrailState>,
    pub covers: FixedPool<NoteHoldCoverState>,
    pub splashes: FixedPool<NoteSplashEntity>,

    pub note_data: Vec<SongNoteData>,
    pub next_note_index: usize,
    has_unhit_notes: bool,
    next_splash_variant: usize,
}

impl Strumline {
    /// Creates a pre-allocated [`Strumline`] positioned at `(x, y)`.
    #[must_use]
    pub fn new(is_player: bool, is_downscroll: bool, scroll_speed: f64, x: f32, y: f32) -> Self {
        let mut strum = Self {
            is_player,
            is_downscroll,
            scroll_speed: scroll_speed.max(0.1),
            x,
            y,
            scale: 1.0,
            note_spacing_scale: 1.0,
            show_notesplash: true,
            custom_render_distance_ms: None,
            ghost_tap_timer_s: 0.0,
            strumline_bg_alpha: 0.0,
            receptors: [
                ReceptorState::new(NoteDirection::Left, 0.0, 0.0),
                ReceptorState::new(NoteDirection::Down, 0.0, 0.0),
                ReceptorState::new(NoteDirection::Up, 0.0, 0.0),
                ReceptorState::new(NoteDirection::Right, 0.0, 0.0),
            ],
            held_keys: HeldKeys::new(),
            notes: FixedPool::with_capacity(NOTE_POOL_CAPACITY, |_| NoteEntity::default()),
            holds: FixedPool::with_capacity(HOLD_POOL_CAPACITY, |_| SustainTrailState::default()),
            covers: FixedPool::with_capacity(HOLD_COVER_POOL_CAPACITY, |_| {
                NoteHoldCoverState::default()
            }),
            splashes: FixedPool::with_capacity(NOTE_SPLASH_CAP, |_| NoteSplashEntity::default()),
            note_data: Vec::new(),
            next_note_index: 0,
            has_unhit_notes: false,
            next_splash_variant: 0,
        };
        strum.layout_receptors();
        strum
    }

    /// Recomputes receptor X/Y positions matching `Strumline.setNoteSpacing`.
    pub fn layout_receptors(&mut self) {
        for (i, dir) in NoteDirection::ALL.iter().enumerate() {
            let rx = self.x + self.get_x_pos(*dir);
            let ry = self.y;
            self.receptors[i].x = rx;
            self.receptors[i].y = ry;
        }
    }

    /// Horizontal offset for lane `dir` (`Strumline.getXPos`).
    #[inline]
    #[must_use]
    pub fn get_x_pos(&self, dir: NoteDirection) -> f32 {
        (dir.as_index() as f32) * NOTE_SPACING * (self.note_spacing_scale * self.scale)
    }

    /// Total width occupied by the 4-lane background bar.
    #[inline]
    #[must_use]
    pub fn width(&self) -> f32 {
        (KEY_COUNT as f32) * NOTE_SPACING * (self.note_spacing_scale * self.scale)
    }

    /// Render lookahead horizon in milliseconds (`Strumline.get_renderDistanceMs`).
    #[inline]
    #[must_use]
    pub fn render_distance_ms(&self) -> f64 {
        if let Some(custom) = self.custom_render_distance_ms {
            return custom;
        }
        let speed_div = if self.scroll_speed < 1.0 {
            self.scroll_speed.max(0.05)
        } else {
            1.0
        };
        f64::from(VIRTUAL_HEIGHT) / PIXELS_PER_MS / speed_div
    }

    /// Computes vertical offset relative to the receptor (`GRhythmUtil.getNoteY`).
    #[inline]
    #[must_use]
    pub fn get_note_y(
        strum_time_ms: f64,
        scroll_speed: f64,
        downscroll: bool,
        time_with_delta_ms: f64,
    ) -> f32 {
        let sign = if downscroll { 1.0 } else { -1.0 };
        (PIXELS_PER_MS * (time_with_delta_ms - strum_time_ms) * scroll_speed * sign) as f32
    }

    /// Loads and pre-sorts chart notes once at song startup (`Strumline.applyNoteData`).
    pub fn apply_note_data(&mut self, notes: &[SongNoteData]) {
        self.clean();
        self.note_data.clear();
        self.note_data.extend(notes.iter().cloned());
        self.note_data.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.next_note_index = 0;
    }

    /// Clears all active pooled entities and resets receptor states without freeing pools.
    pub fn clean(&mut self) {
        self.notes.clear();
        self.holds.clear();
        self.covers.clear();
        self.splashes.clear();
        self.held_keys.clear();
        self.ghost_tap_timer_s = 0.0;
        self.has_unhit_notes = false;
        for receptor in &mut self.receptors {
            receptor.play_static();
        }
    }

    /// Clears pools and resets `next_note_index = 0` when seeking (`handleSkippedNotes`).
    pub fn handle_skipped_notes(&mut self) {
        self.clean();
        self.next_note_index = 0;
    }

    /// Returns whether ghost tapping is currently allowed without penalty.
    #[inline]
    #[must_use]
    pub fn may_ghost_tap(&self) -> bool {
        self.ghost_tap_timer_s > 0.0 || !self.has_unhit_notes
    }

    #[inline]
    pub fn press_key(&mut self, dir: NoteDirection, key_code: KeyCode) {
        self.held_keys.press(dir, key_code);
    }

    #[inline]
    pub fn release_key(&mut self, dir: NoteDirection, key_code: Option<KeyCode>) {
        self.held_keys.release(dir, key_code);
    }

    #[inline]
    #[must_use]
    pub fn is_key_held(&self, dir: NoteDirection) -> bool {
        self.held_keys.is_held(dir)
    }

    #[inline]
    pub fn play_static(&mut self, dir: NoteDirection) {
        self.receptors[dir.as_index()].play_static();
    }

    #[inline]
    pub fn play_press(&mut self, dir: NoteDirection) {
        self.receptors[dir.as_index()].play_press();
    }

    #[inline]
    pub fn play_confirm(&mut self, dir: NoteDirection) {
        self.receptors[dir.as_index()].play_confirm(self.is_player);
    }

    #[inline]
    pub fn hold_confirm(&mut self, dir: NoteDirection) {
        self.receptors[dir.as_index()].hold_confirm();
    }

    /// Collects hittable notes per direction into stack-allocated arrays (zero heap
    /// allocations).
    pub fn collect_notes_may_hit(&self) -> ([[Option<PoolHandle>; 16]; KEY_COUNT], [usize; KEY_COUNT]) {
        let mut buckets: [[Option<PoolHandle>; 16]; KEY_COUNT] = [[None; 16]; KEY_COUNT];
        let mut counts = [0usize; KEY_COUNT];
        for (handle, note) in self.notes.iter_alive() {
            if !note.has_been_hit && note.may_hit {
                let lane = note.direction.as_index();
                if counts[lane] < 16 {
                    buckets[lane][counts[lane]] = Some(handle);
                    counts[lane] += 1;
                }
            }
        }
        (buckets, counts)
    }

    /// Returns `true` if at least one alive unhit note exists on screen.
    #[must_use]
    pub fn has_notes_on_screen(&self) -> bool {
        self.notes.iter_alive().any(|(_, n)| !n.has_been_hit)
    }

    /// Marks `note_handle` as hit, updates its receptor & attached sustain trail,
    /// and resets `ghost_tap_timer_s` (`Strumline.hitNote`).
    pub fn hit_note(
        &mut self,
        note_handle: PoolHandle,
        remove_note: bool,
        song_position_ms: f64,
    ) {
        let Some((dir, hold_handle)) = self.notes.get_mut(note_handle).map(|note| {
            note.has_been_hit = true;
            if !remove_note {
                note.alpha = 0.5;
                note.desaturated = true;
            }
            (note.direction, note.hold_handle)
        }) else {
            return;
        };

        self.play_confirm(dir);

        if remove_note {
            self.kill_note_only(note_handle);
        }

        if let Some(hold_h) = hold_handle {
            if let Some(hold) = self.holds.get_mut(hold_h) {
                hold.hit_note = true;
                hold.missed_note = false;
                hold.frozen_song_time_ms = None;
                hold.alpha = 1.0;
                hold.sustain_ms = hold
                    .full_sustain_ms
                    .min((hold.strum_time_ms + hold.full_sustain_ms) - song_position_ms)
                    .max(0.0);
            }
        }

        self.ghost_tap_timer_s = GHOST_TAP_DELAY;
    }

    /// Releases `note_handle` back to the pool and marks any attached hold note as missed.
    pub fn kill_note(&mut self, note_handle: PoolHandle) {
        let hold_handle = self.notes.get(note_handle).and_then(|n| n.hold_handle);
        self.notes.release(note_handle);

        if let Some(hold_h) = hold_handle {
            if let Some(hold) = self.holds.get_mut(hold_h) {
                hold.missed_note = true;
                hold.visible = false;
            }
        }
    }

    fn kill_note_only(&mut self, note_handle: PoolHandle) {
        self.notes.release(note_handle);
    }

    /// Spawns a recycled note hit splash (`Strumline.playNoteSplash`).
    pub fn play_note_splash(&mut self, dir: NoteDirection, show_enabled: bool) {
        if !self.show_notesplash || !show_enabled {
            return;
        }
        let splash_h = match self.splashes.acquire() {
            Some((h, _)) => h,
            None => {
                // Recycle the oldest active slot cleanly when saturated.
                let first = self.splashes.iter_alive().next().map(|(h, _)| h);
                if let Some(old_h) = first {
                    self.splashes.release(old_h);
                }
                match self.splashes.acquire() {
                    Some((h, _)) => h,
                    None => return,
                }
            }
        };

        let sx = self.x + self.get_x_pos(dir);
        let sy = self.y;
        let variant = self.next_splash_variant;
        self.next_splash_variant = self.next_splash_variant.wrapping_add(1);

        if let Some(splash) = self.splashes.get_mut(splash_h) {
            *splash = NoteSplashEntity {
                direction: dir,
                variant,
                anim_time_s: 0.0,
                frame_index: 0,
                x: sx,
                y: sy,
                visible: true,
            };
        }
    }

    /// Attaches and starts a hold cover for `hold_handle` (`Strumline.playNoteHoldCover`).
    pub fn play_note_hold_cover(&mut self, hold_handle: PoolHandle, enabled: bool) {
        if !self.show_notesplash || !enabled {
            return;
        }
        let Some(dir) = self.holds.get(hold_handle).map(|h| h.direction) else {
            return;
        };

        let cx = self.x + self.get_x_pos(dir);
        let cy = self.y;

        let Some((cover_h, cover)) = self.covers.acquire() else {
            return;
        };

        cover.play_start(dir, hold_handle, cx, cy);

        if let Some(hold) = self.holds.get_mut(hold_handle) {
            hold.cover = Some(cover_h);
        }
    }

    /// Spawns upcoming notes, updates positions/windows/sustains/splashes/covers, and
    /// collects frame gameplay events with **zero heap allocations**.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        elapsed_s: f32,
        song_position_ms: f64,
        time_with_delta_ms: f64,
        start_timestamp_ms: f64,
        is_botplay: bool,
        style: &StrumlineVisualStyle,
    ) -> StrumlineFrameEvents {
        let mut events = StrumlineFrameEvents::default();

        // 1. Spawn notes entering the lookahead window.
        let hit_window_start = song_position_ms - HIT_WINDOW_MS;
        let render_window_start = song_position_ms + self.render_distance_ms();

        while self.next_note_index < self.note_data.len() {
            let note_info = &self.note_data[self.next_note_index];
            if note_info.time < start_timestamp_ms || note_info.time < hit_window_start {
                self.next_note_index += 1;
                continue;
            }
            if note_info.time > render_window_start {
                break;
            }

            let dir = NoteDirection::from_index(note_info.direction(KEY_COUNT))
                .unwrap_or(NoteDirection::Left);
            let strum_time = note_info.time;
            let length = note_info.length.max(0.0);
            let lane_x = self.x + self.get_x_pos(dir);

            let hold_handle = if length > 0.0 {
                let hold_x = lane_x + STRUMLINE_SIZE * 0.5
                    - ((style.hold_tex_size[0] / 8.0) * style.hold_zoom) * 0.5
                    - style.note_nudge;
                if let Some((hh, hold)) = self.holds.acquire() {
                    hold.reset(
                        strum_time,
                        dir,
                        length,
                        true,
                        style.is_pixel,
                        style.hold_zoom,
                        style.hold_tex_size[0],
                        hold_x,
                    );
                    Some(hh)
                } else {
                    None
                }
            } else {
                None
            };

            let note_x = lane_x;
            if let Some((_, note)) = self.notes.acquire() {
                *note = NoteEntity {
                    strum_time_ms: strum_time,
                    sustain_length_ms: length,
                    direction: dir,
                    kind: NoteKindTag::parse(note_info.kind.as_deref()),
                    scoreable: true,
                    low_priority: false,
                    has_been_hit: false,
                    too_early: false,
                    may_hit: false,
                    has_missed: false,
                    handled_miss: false,
                    visible: true,
                    alpha: 1.0,
                    desaturated: false,
                    x: note_x,
                    y: -9999.0,
                    y_offset: 0.0,
                    hold_handle,
                };
                events.spawned_count += 1;
            }

            self.next_note_index += 1;
        }

        // 2. Evaluate hit windows & update note Y positions.
        let controlled_by_player = self.is_player && !is_botplay;
        let mut kill_notes_buf: [Option<PoolHandle>; 32] = [None; 32];
        let mut kill_notes_len = 0usize;

        for (handle, note) in self.notes.iter_alive_mut() {
            note.y = self.y - style.visual_initial_offset
                + Self::get_note_y(
                    note.strum_time_ms,
                    self.scroll_speed,
                    self.is_downscroll,
                    time_with_delta_ms,
                )
                + note.y_offset;

            let outcome = evaluate_note_window(
                note,
                song_position_ms,
                controlled_by_player,
                &mut self.holds,
            );

            if outcome.botplay_hit && events.bot_hit_len < events.bot_hit_handles.len() {
                events.bot_hit_handles[events.bot_hit_len] = Some(handle);
                events.bot_hit_len += 1;
            }

            if outcome.should_continue
                && note.has_missed
                && !note.handled_miss
                && events.newly_missed_len < events.newly_missed_handles.len()
            {
                events.newly_missed_handles[events.newly_missed_len] = Some(handle);
                events.newly_missed_len += 1;
            }

            let is_offscreen = if self.is_downscroll {
                note.y > VIRTUAL_HEIGHT
            } else {
                note.y < -STRUMLINE_SIZE
            };
            if note.handled_miss && is_offscreen && kill_notes_len < kill_notes_buf.len() {
                kill_notes_buf[kill_notes_len] = Some(handle);
                kill_notes_len += 1;
            }
        }

        for handle in kill_notes_buf.iter().take(kill_notes_len).flatten() {
            self.kill_note(*handle);
        }

        // 3. Update sustain trails (clipping, release drops, completion).
        let mut kill_holds_buf: [Option<PoolHandle>; 32] = [None; 32];
        let mut kill_holds_len = 0usize;
        let render_dist = self.render_distance_ms();

        for (handle, hold) in self.holds.iter_alive_mut() {
            if song_position_ms > hold.strum_time_ms
                && hold.hit_note
                && !hold.missed_note
                && self.is_player
                && !self.held_keys.is_held(hold.direction)
            {
                self.receptors[hold.direction.as_index()].play_static();
                hold.missed_note = true;
                hold.frozen_song_time_ms = Some(song_position_ms);
                hold.visible = true;
                hold.alpha = 0.0;

                if !hold.handled_miss && events.hold_drop_len < events.hold_drop_handles.len() {
                    events.hold_drop_handles[events.hold_drop_len] = Some(handle);
                    events.hold_drop_len += 1;
                }
            }

            let render_window_end =
                hold.strum_time_ms + hold.full_sustain_ms + HIT_WINDOW_MS + (render_dist / 8.0);

            if hold.missed_note && song_position_ms >= render_window_end {
                hold.visible = false;
                if let Some(cover_h) = hold.cover.take() {
                    if let Some(cov) = self.covers.get_mut(cover_h) {
                        cov.visible = false;
                    }
                    self.covers.release(cover_h);
                }
                if kill_holds_len < kill_holds_buf.len() {
                    kill_holds_buf[kill_holds_len] = Some(handle);
                    kill_holds_len += 1;
                }
                continue;
            }

            if hold.hit_note && !hold.missed_note && hold.sustain_ms <= 0.0 {
                let dir = hold.direction;
                if self.held_keys.is_held(dir) {
                    self.receptors[dir.as_index()].play_press();
                } else {
                    self.receptors[dir.as_index()].play_static();
                }
                if let Some(cover_h) = hold.cover {
                    if self.is_player {
                        if let Some(cov) = self.covers.get_mut(cover_h) {
                            cov.play_end();
                        }
                    } else {
                        if let Some(cov) = self.covers.get_mut(cover_h) {
                            cov.visible = false;
                        }
                        self.covers.release(cover_h);
                        hold.cover = None;
                    }
                }
                hold.visible = false;
                if kill_holds_len < kill_holds_buf.len() {
                    kill_holds_buf[kill_holds_len] = Some(handle);
                    kill_holds_len += 1;
                }
                continue;
            }

            if hold.missed_note && hold.full_sustain_ms > hold.sustain_ms {
                hold.visible = true;
                if let Some(cover_h) = hold.cover.take() {
                    if let Some(cov) = self.covers.get_mut(cover_h) {
                        cov.visible = false;
                    }
                    self.covers.release(cover_h);
                }
            } else if song_position_ms > hold.strum_time_ms && hold.hit_note {
                self.receptors[hold.direction.as_index()].hold_confirm();
                hold.visible = true;
                hold.sustain_ms =
                    ((hold.strum_time_ms + hold.full_sustain_ms) - song_position_ms).max(0.0);
                if hold.sustain_ms <= 10.0 {
                    hold.visible = false;
                }
                if hold.scoreable && hold.sustain_ms > 0.0 {
                    events.active_hold_scoreable_count += 1;
                }
            } else {
                hold.visible = true;
            }

            hold.y = hold.compute_screen_y(
                self.y,
                style.visual_initial_offset,
                self.scroll_speed,
                self.is_downscroll,
                song_position_ms,
                time_with_delta_ms,
            );
        }

        for handle in kill_holds_buf.iter().take(kill_holds_len).flatten() {
            self.holds.release(*handle);
        }

        // 4. Advance receptors, splashes, covers, and ghost-tap timer.
        for (i, receptor) in self.receptors.iter_mut().enumerate() {
            receptor.update(elapsed_s, self.is_player, style.confirm_frames);
            if let Some(dir) = NoteDirection::from_index(i) {
                if self.held_keys.is_held(dir) && receptor.anim == ReceptorAnim::Static {
                    receptor.play_press();
                }
            }
        }

        let mut dead_splashes: [Option<PoolHandle>; NOTE_SPLASH_CAP] = [None; NOTE_SPLASH_CAP];
        let mut dead_splash_len = 0usize;
        for (handle, splash) in self.splashes.iter_alive_mut() {
            splash.anim_time_s += elapsed_s.max(0.0);
            let frame = (splash.anim_time_s * NOTE_ANIM_FPS).floor() as usize;
            if frame >= style.splash_frames.max(1) {
                splash.visible = false;
                if dead_splash_len < dead_splashes.len() {
                    dead_splashes[dead_splash_len] = Some(handle);
                    dead_splash_len += 1;
                }
            } else {
                splash.frame_index = frame;
            }
        }
        for handle in dead_splashes.iter().take(dead_splash_len).flatten() {
            self.splashes.release(*handle);
        }

        advance_hold_covers(
            &mut self.covers,
            &mut self.holds,
            elapsed_s,
            style.cover_start_frames,
            style.cover_loop_frames,
            style.cover_end_frames,
        );

        self.has_unhit_notes = self.has_notes_on_screen();
        if !self.has_unhit_notes {
            self.ghost_tap_timer_s = (self.ghost_tap_timer_s - f64::from(elapsed_s)).max(0.0);
        }

        events
    }

    /// Queues all strumline elements across depth-ordered passes (`bg_batcher` for
    /// background & receptors, `mesh_batcher` for sustain trails in front of receptors,
    /// `fg_batcher` for note heads, hold covers, and splashes) with zero heap allocations.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        bg_batcher: &mut SpriteBatcher,
        fg_batcher: &mut SpriteBatcher,
        mesh_batcher: &mut MeshBatcher,
        style: &StrumlineVisualStyle,
        note_assets: Option<&NoteStyleAssets>,
        song_position_ms: f64,
    ) {
        let filter = if style.is_pixel {
            FilterMode::Nearest
        } else {
            FilterMode::Linear
        };
        let fallback_tex = style.note_texture.unwrap_or(style.white_texture);

        // 0. Optional strumline lane background (`zIndex < 10`).
        if self.strumline_bg_alpha > 0.0 {
            const BACKGROUND_PAD: f32 = 16.0;
            bg_batcher.push(
                style.white_texture,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new(
                    [self.x - BACKGROUND_PAD, 0.0],
                    [self.width() + BACKGROUND_PAD * 2.0, VIRTUAL_HEIGHT],
                )
                .with_color([0.0, 0.0, 0.0, self.strumline_bg_alpha]),
            );
        }

        // 1. Receptors (`zIndex 10`, behind sustain trails).
        let sparrow_strum = note_assets.filter(|na| na.has_sparrow_strum());
        for receptor in &self.receptors {
            if let Some(na) = sparrow_strum {
                if let Some((tex, tex_size)) = na.strum_tex {
                    let lane = receptor.direction.as_index();
                    let frames = match receptor.anim {
                        ReceptorAnim::Static => &na.strum_static[lane],
                        ReceptorAnim::Press => &na.strum_press[lane],
                        ReceptorAnim::Confirm => &na.strum_confirm[lane],
                        ReceptorAnim::ConfirmHold => {
                            if na.strum_confirm_hold[lane].is_empty() {
                                &na.strum_confirm[lane]
                            } else {
                                &na.strum_confirm_hold[lane]
                            }
                        }
                    };
                    // V-Slice receptor animations default to `looped: false`: hold the
                    // final frame rather than wrapping back to frame 0 cleanly.
                    if let Some(frame) = frames
                        .get(receptor.anim_frame.min(frames.len().saturating_sub(1)))
                        .or_else(|| frames.first())
                    {
                        let params = SparrowDrawParams {
                            texture: tex,
                            tex_size,
                            pos: [
                                receptor.x + style.visual_initial_offset + na.strum_offsets[0],
                                receptor.y + na.strum_offsets[1],
                            ],
                            scale: na.strum_scale * self.scale,
                            hitbox_size: [
                                na.strum_hitbox[0] * self.scale,
                                na.strum_hitbox[1] * self.scale,
                            ],
                            extra_offset: [0.0, 0.0],
                            color: [1.0, 1.0, 1.0, receptor.alpha],
                            hsv_shift: [0.0; 3],
                            blend: BlendMode::Normal,
                            filter: if na.strum_pixel {
                                FilterMode::Nearest
                            } else {
                                FilterMode::Linear
                            },
                            pixel_snap: false,
                        };
                        push_sparrow_frame(bg_batcher, frame, &params);
                        continue;
                    }
                }
            }

            let base = StrumlineVisualStyle::lane_color(receptor.direction);
            let (size, color, pos_offset) = match receptor.anim {
                ReceptorAnim::Static => (
                    STRUMLINE_SIZE * 0.88,
                    [0.62, 0.65, 0.72, 0.75 * receptor.alpha],
                    STRUMLINE_SIZE * 0.06,
                ),
                ReceptorAnim::Press => (
                    STRUMLINE_SIZE * 0.80,
                    [0.92, 0.94, 1.0, 0.90 * receptor.alpha],
                    STRUMLINE_SIZE * 0.10,
                ),
                ReceptorAnim::Confirm | ReceptorAnim::ConfirmHold => (
                    STRUMLINE_SIZE * 0.96,
                    [base[0], base[1], base[2], receptor.alpha],
                    STRUMLINE_SIZE * 0.02,
                ),
            };
            bg_batcher.push(
                fallback_tex,
                BlendMode::Normal,
                filter,
                SpriteInstance::new([receptor.x + pos_offset, receptor.y + pos_offset], [size, size])
                    .with_color(color),
            );
        }

        // 2. Clipped sustain trails (`zIndex 20`, render in front of receptors and behind note heads).
        if let Some(hold_tex) = style.hold_texture {
            for (_, hold) in self.holds.iter_alive() {
                hold.queue_mesh(
                    mesh_batcher,
                    hold_tex,
                    style.hold_tex_size[0],
                    style.hold_tex_size[1],
                    song_position_ms,
                    self.scroll_speed,
                    self.is_downscroll,
                    style.hold_offsets,
                    hold.y,
                );
            }
        } else {
            for (_, hold) in self.holds.iter_alive() {
                if !hold.visible || hold.alpha <= 0.0 {
                    continue;
                }
                let clip_h = hold.current_clip_height(song_position_ms, self.scroll_speed);
                if clip_h <= 1.0 {
                    continue;
                }
                let base = StrumlineVisualStyle::lane_color(hold.direction);
                let w = (STRUMLINE_SIZE * 0.28).max(12.0);
                let cx = hold.x + (hold.graphic_width - w) * 0.5;
                let bar_y =
                    if !self.is_downscroll && hold.hit_note && song_position_ms > hold.strum_time_ms {
                        self.y - style.visual_initial_offset + STRUMLINE_SIZE * 0.5
                    } else {
                        hold.y
                    };
                fg_batcher.push(
                    style.white_texture,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([cx, bar_y], [w, clip_h])
                        .with_color([base[0], base[1], base[2], 0.65 * hold.alpha]),
                );
            }
        }

        // 3. Note heads (`zIndex 30`).
        let sparrow_notes = note_assets.filter(|na| na.has_sparrow_notes());
        for (_, note) in self.notes.iter_alive() {
            if !note.visible || note.alpha <= 0.0 {
                continue;
            }
            if let Some(na) = sparrow_notes {
                if let Some((tex, tex_size)) = na.note_tex {
                    let lane = note.direction.as_index();
                    if let Some(frame) = na.note_frames[lane].first() {
                        let hitbox_w = na.note_hitbox[lane][0] * self.scale;
                        let hitbox_h = na.note_hitbox[lane][1] * self.scale;
                        let params = SparrowDrawParams {
                            texture: tex,
                            tex_size,
                            pos: [
                                note.x - (hitbox_w - STRUMLINE_SIZE) * 0.5 - style.note_nudge
                                    + na.note_offsets[0],
                                note.y + na.note_offsets[1],
                            ],
                            scale: na.note_scale * self.scale,
                            hitbox_size: [hitbox_w, hitbox_h],
                            extra_offset: [0.0, 0.0],
                            color: [1.0, 1.0, 1.0, note.alpha],
                            hsv_shift: if note.desaturated {
                                [0.0, -0.8, 0.0]
                            } else {
                                [0.0; 3]
                            },
                            blend: BlendMode::Normal,
                            filter: if na.note_pixel {
                                FilterMode::Nearest
                            } else {
                                FilterMode::Linear
                            },
                            pixel_snap: false,
                        };
                        push_sparrow_frame(fg_batcher, frame, &params);
                        continue;
                    }
                }
            }

            let base = StrumlineVisualStyle::lane_color(note.direction);
            let size = STRUMLINE_SIZE * 0.88 * self.scale;
            let pad = (STRUMLINE_SIZE - size) * 0.5;
            let mut inst = SpriteInstance::new([note.x + pad, note.y + pad], [size, size])
                .with_color([base[0], base[1], base[2], note.alpha]);
            if note.desaturated {
                inst = inst.with_hsv_shift([0.0, -0.8, 0.0]);
            }
            fg_batcher.push(fallback_tex, BlendMode::Normal, filter, inst);
        }

        // 4. Hold covers (`zIndex 40`).
        for (_, cover) in self.covers.iter_alive() {
            if !cover.visible || cover.alpha <= 0.0 {
                continue;
            }
            if let Some(na) = note_assets.filter(|a| a.has_cover_for(cover.direction)) {
                let lane = cover.direction.as_index();
                if let Some((tex, tex_size)) = na.cover_tex[lane] {
                    let frames = match cover.phase {
                        HoldCoverPhase::Start => &na.cover_start[lane],
                        HoldCoverPhase::Loop => &na.cover_loop[lane],
                        HoldCoverPhase::End => &na.cover_end[lane],
                        HoldCoverPhase::Hidden => &na.cover_loop[lane],
                    };
                    if let Some(frame) = frames
                        .get(cover.frame_index % frames.len().max(1))
                        .or_else(|| frames.first())
                    {
                        let hb = na.cover_hitbox;
                        let params = SparrowDrawParams {
                            texture: tex,
                            tex_size,
                            pos: [
                                cover.x + STRUMLINE_SIZE * 0.5 - hb[0] * 0.5 - 12.0
                                    + na.cover_offsets[0] * na.cover_scale,
                                cover.y + style.visual_initial_offset + STRUMLINE_SIZE * 0.5
                                    - 96.0
                                    + na.cover_offsets[1] * na.cover_scale,
                            ],
                            scale: na.cover_scale,
                            hitbox_size: hb,
                            extra_offset: [0.0, 0.0],
                            color: [1.0, 1.0, 1.0, cover.alpha],
                            hsv_shift: [0.0; 3],
                            blend: BlendMode::Normal,
                            filter: if na.cover_pixel {
                                FilterMode::Nearest
                            } else {
                                FilterMode::Linear
                            },
                            pixel_snap: false,
                        };
                        push_sparrow_frame(fg_batcher, frame, &params);
                        continue;
                    }
                }
            }

            if note_assets.is_some_and(|na| !na.cover_enabled) {
                continue;
            }
            let base = StrumlineVisualStyle::lane_color(cover.direction);
            let size = STRUMLINE_SIZE * 0.9;
            let pad = (STRUMLINE_SIZE - size) * 0.5;
            fg_batcher.push(
                style.white_texture,
                BlendMode::Add,
                filter,
                SpriteInstance::new([cover.x + pad, cover.y + pad], [size, size])
                    .with_color([base[0], base[1], base[2], 0.55 * cover.alpha]),
            );
        }

        // 5. Note splashes (`zIndex 50`).
        let sparrow_splashes = note_assets.filter(|na| na.has_sparrow_splashes());
        for (_, splash) in self.splashes.iter_alive() {
            if !splash.visible {
                continue;
            }
            if let Some(na) = sparrow_splashes {
                if let Some((tex, tex_size)) = na.splash_tex {
                    let lane = splash.direction.as_index();
                    let variants = &na.splash_variants[lane];
                    if !variants.is_empty() {
                        let var_frames = &variants[splash.variant % variants.len()];
                        if let Some(frame) = var_frames.get(splash.frame_index) {
                            let src = frame.source_size();
                            let scaled = [src[0] * na.splash_scale, src[1] * na.splash_scale];
                            // NoteSplash overwrites `offset.set(width * 0.3, height * 0.3)` after
                            // `updateHitbox()`, while FlxSprite keeps scaling around `src * 0.5`.
                            let scale_origin_shift = [
                                src[0] * 0.5 * (na.splash_scale - 1.0),
                                src[1] * 0.5 * (na.splash_scale - 1.0),
                            ];
                            let params = SparrowDrawParams {
                                texture: tex,
                                tex_size,
                                pos: [
                                    splash.x
                                        + style.visual_initial_offset
                                        + na.splash_offsets[0] * na.splash_scale,
                                    splash.y - style.visual_initial_offset
                                        + na.splash_offsets[1] * na.splash_scale,
                                ],
                                scale: na.splash_scale,
                                hitbox_size: scaled,
                                extra_offset: [
                                    na.splash_hitbox[0] * 0.3 + scale_origin_shift[0],
                                    na.splash_hitbox[1] * 0.3 + scale_origin_shift[1],
                                ],
                                color: [1.0, 1.0, 1.0, na.splash_alpha],
                                hsv_shift: [0.0; 3],
                                blend: na.splash_blend,
                                filter: if na.splash_pixel {
                                    FilterMode::Nearest
                                } else {
                                    FilterMode::Linear
                                },
                                pixel_snap: false,
                            };
                            push_sparrow_frame(fg_batcher, frame, &params);
                            continue;
                        }
                    }
                }
            }

            let progress =
                (splash.anim_time_s * NOTE_ANIM_FPS / style.splash_frames.max(1) as f32).clamp(0.0, 1.0);
            let grow = STRUMLINE_SIZE * (1.0 + 0.55 * progress);
            let shift = (grow - STRUMLINE_SIZE) * 0.5;
            let base = StrumlineVisualStyle::lane_color(splash.direction);
            fg_batcher.push(
                style.white_texture,
                BlendMode::Add,
                filter,
                SpriteInstance::new([splash.x - shift, splash.y - shift], [grow, grow])
                    .with_color([base[0], base[1], base[2], (1.0 - progress) * 0.7]),
            );
        }
    }
}

/// Evaluates `HIT_WINDOW_MS` for a single note (`GRhythmUtil.processWindow`).
pub fn evaluate_note_window(
    note: &mut NoteEntity,
    song_position_ms: f64,
    is_controlled: bool,
    holds: &mut FixedPool<SustainTrailState>,
) -> HitWindowOutcome {
    if note.has_missed || note.has_been_hit {
        return HitWindowOutcome {
            botplay_hit: false,
            should_continue: false,
        };
    }

    let window_start = note.strum_time_ms - HIT_WINDOW_MS;
    let window_center = note.strum_time_ms;
    let window_end = note.strum_time_ms + HIT_WINDOW_MS;

    if song_position_ms > window_end {
        note.too_early = false;
        note.has_missed = true;
        note.may_hit = false;
        if let Some(hold_h) = note.hold_handle {
            if let Some(hold) = holds.get_mut(hold_h) {
                hold.missed_note = true;
            }
        }
        return HitWindowOutcome {
            botplay_hit: false,
            should_continue: true,
        };
    }

    if !is_controlled && song_position_ms >= window_center {
        return HitWindowOutcome {
            botplay_hit: true,
            should_continue: true,
        };
    }

    if let Some(hold_h) = note.hold_handle {
        if let Some(hold) = holds.get_mut(hold_h) {
            hold.missed_note = false;
        }
    }

    if song_position_ms >= window_start {
        note.too_early = false;
        note.has_missed = false;
        note.may_hit = true;
    } else {
        note.too_early = true;
        note.may_hit = false;
        note.has_missed = false;
    }

    HitWindowOutcome {
        botplay_hit: false,
        should_continue: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strumline_spawns_evaluates_and_hits_note_without_realloc() {
        let mut strum = Strumline::new(true, false, 1.0, 688.0, 24.0);
        strum.apply_note_data(&[SongNoteData {
            time: 1000.0,
            data: 2, // Player Up
            length: 0.0,
            kind: None,
            params: Vec::new(),
        }]);

        let style = StrumlineVisualStyle::fallback(TextureHandle(0));

        // Frame at 500ms: note enters render distance (~1600ms) and spawns.
        let ev0 = strum.update(1.0 / 60.0, 500.0, 500.0, 0.0, false, &style);
        assert_eq!(ev0.spawned_count, 1);
        assert_eq!(strum.notes.active_count(), 1);

        // Frame at 960ms: note is inside 160ms hit window.
        strum.update(1.0 / 60.0, 960.0, 960.0, 0.0, false, &style);
        let (buckets, counts) = strum.collect_notes_may_hit();
        assert_eq!(counts[NoteDirection::Up.as_index()], 1);
        let target = buckets[NoteDirection::Up.as_index()][0].expect("target");

        strum.hit_note(target, true, 960.0);
        assert_eq!(strum.notes.active_count(), 0);
        assert_eq!(
            strum.receptors[NoteDirection::Up.as_index()].anim,
            ReceptorAnim::Confirm
        );
    }

    #[test]
    fn downscroll_flips_note_y_direction() {
        let up_y = Strumline::get_note_y(1000.0, 1.0, false, 500.0);
        let down_y = Strumline::get_note_y(1000.0, 1.0, true, 500.0);
        assert!((up_y - 225.0).abs() < 1e-4);
        assert!((down_y - (-225.0)).abs() < 1e-4);
    }
}
