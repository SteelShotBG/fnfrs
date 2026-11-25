//! Pooled V-Slice sustain trail & hold cover state machines matching
//! `SustainTrail.hx` and `NoteHoldCover.hx`.

use fnfrs_core::constants::{
    HEALTH_HOLD_DROP_PENALTY_MAX, HEALTH_HOLD_DROP_PENALTY_PER_SECOND, HEALTH_MISS_PENALTY,
    HOLD_DROP_PENALTY_THRESHOLD_MS, INITIAL_OFFSET, PIXELS_PER_MS,
    SCORE_HOLD_DROP_PENALTY_PER_SECOND, STRUMLINE_SIZE,
};
use fnfrs_core::pool::{FixedPool, PoolHandle};
use fnfrs_render::{BlendMode, FilterMode, MeshBatcher, SustainTrailParams, TextureHandle};

use crate::input::NoteDirection;

/// Default frame rate for hold cover Sparrow animations (`NoteHoldCover.FRAMERATE_DEFAULT`).
pub const HOLD_COVER_FPS: f32 = 24.0;

/// Visual lifecycle phase for a pooled [`NoteHoldCoverState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldCoverPhase {
    #[default]
    Hidden,
    Start,
    Loop,
    End,
}

/// Pooled sustain hold cover (`NoteHoldCover.hx`).
#[derive(Debug, Clone, Default)]
pub struct NoteHoldCoverState {
    pub direction: NoteDirection,
    pub phase: HoldCoverPhase,
    pub anim_time_s: f32,
    pub frame_index: usize,
    pub x: f32,
    pub y: f32,
    pub visible: bool,
    pub alpha: f32,
    pub owner_hold: Option<PoolHandle>,
}

impl NoteHoldCoverState {
    /// Resets and starts playing the `holdCoverStart<Color>` animation.
    pub fn play_start(&mut self, dir: NoteDirection, owner_hold: PoolHandle, x: f32, y: f32) {
        self.direction = dir;
        self.phase = HoldCoverPhase::Start;
        self.anim_time_s = 0.0;
        self.frame_index = 0;
        self.x = x;
        self.y = y;
        self.visible = true;
        self.alpha = 1.0;
        self.owner_hold = Some(owner_hold);
    }

    /// Transitions immediately to the `holdCover<Color>` loop animation.
    pub fn play_continue(&mut self) {
        self.phase = HoldCoverPhase::Loop;
        self.anim_time_s = 0.0;
        self.frame_index = 0;
    }

    /// Plays the `holdCoverEnd<Color>` zap/end animation.
    pub fn play_end(&mut self) {
        if self.phase != HoldCoverPhase::End && self.phase != HoldCoverPhase::Hidden {
            self.phase = HoldCoverPhase::End;
            self.anim_time_s = 0.0;
            self.frame_index = 0;
        }
    }

    /// Advances the cover animation state machine; returns `true` when finished and
    /// ready to be released back to the [`FixedPool`].
    pub fn advance(
        &mut self,
        elapsed_s: f32,
        start_frames: usize,
        loop_frames: usize,
        end_frames: usize,
    ) -> bool {
        if !self.visible || self.phase == HoldCoverPhase::Hidden {
            return false;
        }

        self.anim_time_s += elapsed_s.max(0.0);
        let raw_frame = (self.anim_time_s * HOLD_COVER_FPS).floor() as usize;

        match self.phase {
            HoldCoverPhase::Hidden => false,
            HoldCoverPhase::Start => {
                let count = start_frames.max(1);
                if raw_frame >= count {
                    self.play_continue();
                } else {
                    self.frame_index = raw_frame;
                }
                false
            }
            HoldCoverPhase::Loop => {
                let count = loop_frames.max(1);
                self.frame_index = raw_frame % count;
                false
            }
            HoldCoverPhase::End => {
                let count = end_frames.max(1);
                if raw_frame >= count {
                    self.visible = false;
                    self.phase = HoldCoverPhase::Hidden;
                    true
                } else {
                    self.frame_index = raw_frame;
                    false
                }
            }
        }
    }
}

/// Pooled sustain trail entity (`SustainTrail.hx`) with pre-allocated slot state.
#[derive(Debug, Clone)]
pub struct SustainTrailState {
    pub strum_time_ms: f64,
    pub direction: NoteDirection,
    pub full_sustain_ms: f64,
    pub sustain_ms: f64,
    /// Snapshot of `song_position_ms` taken if the player drops a partially held trail.
    pub frozen_song_time_ms: Option<f64>,
    pub y_offset: f32,
    pub hit_note: bool,
    pub missed_note: bool,
    pub handled_miss: bool,
    pub scoreable: bool,
    pub visible: bool,
    pub alpha: f32,
    pub is_pixel: bool,
    pub zoom: f32,
    pub graphic_width: f32,
    pub x: f32,
    pub y: f32,
    pub cover: Option<PoolHandle>,
}

impl Default for SustainTrailState {
    fn default() -> Self {
        Self {
            strum_time_ms: 0.0,
            direction: NoteDirection::Left,
            full_sustain_ms: 0.0,
            sustain_ms: 0.0,
            frozen_song_time_ms: None,
            y_offset: 0.0,
            hit_note: false,
            missed_note: false,
            handled_miss: false,
            scoreable: true,
            visible: true,
            alpha: 1.0,
            is_pixel: false,
            zoom: 1.0,
            graphic_width: 0.0,
            x: 0.0,
            y: -9999.0,
            cover: None,
        }
    }
}

impl SustainTrailState {
    /// Resets a recycled pool slot for a new hold note (`Strumline.buildHoldNoteSprite`).
    #[allow(clippy::too_many_arguments)]
    pub fn reset(
        &mut self,
        strum_time_ms: f64,
        direction: NoteDirection,
        full_sustain_ms: f64,
        scoreable: bool,
        is_pixel: bool,
        zoom: f32,
        tex_width: f32,
        x: f32,
    ) {
        self.strum_time_ms = strum_time_ms;
        self.direction = direction;
        self.full_sustain_ms = full_sustain_ms.max(0.0);
        self.sustain_ms = self.full_sustain_ms;
        self.frozen_song_time_ms = None;
        self.y_offset = 0.0;
        self.hit_note = false;
        self.missed_note = false;
        self.handled_miss = false;
        self.scoreable = scoreable;
        self.visible = true;
        self.alpha = 1.0;
        self.is_pixel = is_pixel;
        self.zoom = zoom;
        self.graphic_width = (tex_width / 8.0) * zoom;
        self.x = x;
        self.y = -9999.0;
        self.cover = None;
    }

    /// Computes sustain height in pixels for a duration and scroll speed
    /// (`SustainTrail.sustainHeight`).
    #[inline]
    #[must_use]
    pub fn sustain_height(sustain_ms: f64, scroll_speed: f64) -> f32 {
        (sustain_ms.max(0.0) * PIXELS_PER_MS * scroll_speed.max(0.0)) as f32
    }

    /// Clipped visual height at `song_time_ms`.
    #[inline]
    #[must_use]
    pub fn current_clip_height(&self, song_time_ms: f64, scroll_speed: f64) -> f32 {
        let effective_time = self.frozen_song_time_ms.unwrap_or(song_time_ms);
        let elapsed_hold = effective_time - self.strum_time_ms;
        let remaining = (self.full_sustain_ms - elapsed_hold).clamp(0.0, self.full_sustain_ms);
        Self::sustain_height(remaining, scroll_speed)
    }

    /// Updates screen-space `y`, clipping progress, and visibility for the current frame.
    ///
    /// Returns `true` if `MeshBatcher::push_sustain_trail` should render this trail.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_screen_y(
        &self,
        strumline_y: f32,
        scroll_speed: f64,
        is_downscroll: bool,
        song_position_ms: f64,
        time_with_delta_ms: f64,
    ) -> f32 {
        let clip_height = self.current_clip_height(song_position_ms, scroll_speed);
        let note_y_delta =
            (PIXELS_PER_MS * (time_with_delta_ms - self.strum_time_ms) * scroll_speed) as f32
                * if is_downscroll { 1.0 } else { -1.0 };

        if self.missed_note && self.full_sustain_ms > self.sustain_ms {
            let dropped_px =
                ((self.full_sustain_ms - self.sustain_ms) * PIXELS_PER_MS) as f32;
            if is_downscroll {
                strumline_y - INITIAL_OFFSET + note_y_delta - clip_height + STRUMLINE_SIZE * 0.5
                    + self.y_offset
            } else {
                strumline_y - INITIAL_OFFSET + note_y_delta + dropped_px + STRUMLINE_SIZE * 0.5
                    + self.y_offset
            }
        } else if song_position_ms > self.strum_time_ms && self.hit_note {
            if is_downscroll {
                strumline_y - INITIAL_OFFSET - clip_height + STRUMLINE_SIZE * 0.5
            } else {
                strumline_y - INITIAL_OFFSET + STRUMLINE_SIZE * 0.5
            }
        } else if is_downscroll {
            strumline_y - INITIAL_OFFSET + note_y_delta - clip_height + STRUMLINE_SIZE * 0.5
                + self.y_offset
        } else {
            strumline_y - INITIAL_OFFSET + note_y_delta + STRUMLINE_SIZE * 0.5 + self.y_offset
        }
    }

    /// Emits this sustain trail's clipped geometry into `mesh_batcher` with zero
    /// heap allocations.
    #[allow(clippy::too_many_arguments)]
    pub fn queue_mesh(
        &self,
        mesh_batcher: &mut MeshBatcher,
        texture: TextureHandle,
        tex_width: f32,
        tex_height: f32,
        song_time_ms: f64,
        scroll_speed: f64,
        is_downscroll: bool,
        style_offsets: [f32; 2],
        draw_y: f32,
    ) {
        if !self.visible || self.alpha <= 0.0 || self.sustain_ms <= 0.0 {
            return;
        }

        // Keep `sustain_ms = full_sustain_ms` and feed either active `song_time_ms` or
        // `frozen_song_time_ms` because `push_sustain_trail` subtracts
        // `(song_time_ms - strum_time_ms)` directly against `sustain_ms`.
        let effective_song_time = if self.missed_note {
            self.frozen_song_time_ms.unwrap_or(self.strum_time_ms)
        } else if self.hit_note {
            song_time_ms
        } else {
            self.strum_time_ms
        };

        let filter = if self.is_pixel {
            FilterMode::Nearest
        } else {
            FilterMode::Linear
        };

        let params = SustainTrailParams {
            x: self.x - style_offsets[0],
            y: draw_y - style_offsets[1],
            lane: self.direction.as_index(),
            full_sustain_ms: self.full_sustain_ms,
            sustain_ms: self.full_sustain_ms,
            song_time_ms: effective_song_time,
            strum_time_ms: self.strum_time_ms,
            scroll_speed,
            tex_width,
            tex_height,
            zoom: self.zoom,
            is_pixel: self.is_pixel,
            flip_y: is_downscroll,
            color: [1.0, 1.0, 1.0, self.alpha],
        };

        mesh_batcher.push_sustain_trail(texture, BlendMode::Normal, filter, &params);
    }
}

/// Score & health penalty computed when dropping an active sustain note early
/// (`PlayState.processNotes`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoldDropPenalty {
    pub should_penalize: bool,
    pub health_change: f32,
    pub score_change: f64,
}

/// Computes V-Slice hold-drop penalty for `remaining_ms` and whether base note `was_hit`.
#[must_use]
pub fn compute_hold_drop_penalty(remaining_ms: f64, was_hit: bool) -> HoldDropPenalty {
    if remaining_ms <= HOLD_DROP_PENALTY_THRESHOLD_MS {
        return HoldDropPenalty {
            should_penalize: false,
            health_change: 0.0,
            score_change: 0.0,
        };
    }

    let remaining_sec = remaining_ms / 1000.0;
    let uncapped_health = (remaining_sec as f32) * HEALTH_HOLD_DROP_PENALTY_PER_SECOND;
    let max_penalty = HEALTH_HOLD_DROP_PENALTY_MAX
        - if was_hit {
            -HEALTH_MISS_PENALTY
        } else {
            0.0
        };
    let health_change = uncapped_health.clamp(max_penalty, 0.0);
    let score_change = f64::from(SCORE_HOLD_DROP_PENALTY_PER_SECOND) * remaining_sec;

    HoldDropPenalty {
        should_penalize: true,
        health_change,
        score_change,
    }
}

/// Updates all active covers in `covers`, releasing completed `End` covers without allocating.
pub fn advance_hold_covers(
    covers: &mut FixedPool<NoteHoldCoverState>,
    holds: &mut FixedPool<SustainTrailState>,
    elapsed_s: f32,
    start_frames: usize,
    loop_frames: usize,
    end_frames: usize,
) {
    let mut finished_buf: [Option<(PoolHandle, Option<PoolHandle>)>; 32] = [None; 32];
    let mut finished_len = 0usize;

    for (handle, cover) in covers.iter_alive_mut() {
        if cover.advance(elapsed_s, start_frames, loop_frames, end_frames)
            && finished_len < finished_buf.len()
        {
            finished_buf[finished_len] = Some((handle, cover.owner_hold));
            finished_len += 1;
        }
    }

    for entry in finished_buf.iter().take(finished_len).flatten() {
        let (cover_handle, owner_hold) = *entry;
        if let Some(hold_handle) = owner_hold {
            if let Some(hold) = holds.get_mut(hold_handle) {
                if hold.cover == Some(cover_handle) {
                    hold.cover = None;
                }
            }
        }
        covers.release(cover_handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_drop_penalty_matches_vslice_thresholds() {
        let short = compute_hold_drop_penalty(160.0, true);
        assert!(!short.should_penalize);

        let long = compute_hold_drop_penalty(1000.0, true);
        assert!(long.should_penalize);
        assert!(long.health_change < 0.0);
        assert!((long.score_change - (-125.0)).abs() < 1e-6);
    }

    #[test]
    fn sustain_clip_tracks_elapsed_hold() {
        let mut hold = SustainTrailState::default();
        hold.reset(1000.0, NoteDirection::Down, 500.0, true, false, 0.7, 408.0, 200.0);
        hold.hit_note = true;

        let full_h = hold.current_clip_height(1000.0, 1.0);
        let half_h = hold.current_clip_height(1250.0, 1.0);
        assert!((full_h - 225.0).abs() < 1e-4);
        assert!((half_h - 112.5).abs() < 1e-4);
        assert!((hold.current_clip_height(1600.0, 1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn hold_cover_state_machine_cycles_start_loop_end() {
        let mut holds = FixedPool::with_capacity(4, |_| SustainTrailState::default());
        let mut covers = FixedPool::with_capacity(4, |_| NoteHoldCoverState::default());

        let (hold_h, _) = holds.acquire().expect("hold");
        let (cover_h, cover) = covers.acquire().expect("cover");
        cover.play_start(NoteDirection::Up, hold_h, 100.0, 24.0);
        holds.get_mut(hold_h).expect("hold").cover = Some(cover_h);

        // Advance past 3 start frames at 24 fps (~0.15s).
        advance_hold_covers(&mut covers, &mut holds, 0.15, 3, 4, 3);
        assert_eq!(
            covers.get(cover_h).expect("cover").phase,
            HoldCoverPhase::Loop
        );

        covers.get_mut(cover_h).expect("cover").play_end();
        advance_hold_covers(&mut covers, &mut holds, 0.20, 3, 4, 3);
        assert_eq!(covers.active_count(), 0);
        assert!(holds.get(hold_h).expect("hold").cover.is_none());
    }
}
