//! V-Slice pre-song countdown (`Countdown.hx`, `PlayState.hx`, `NoteStyle.hx`).
//!
//! Runs a negative song-time pre-roll (`-4 * beat_length_ms` up to `0`),
//! ticking `Three -> Two (ready) -> One (set) -> Go -> Done`, emitting SFX cues
//! (`intro3`, `intro2`, `intro1`, `introGo`), and drawing `ready.png`, `set.png`,
//! `go.png` with `FlxEase.cubeInOut` (or 8-step fade for pixel styles) with zero
//! per-frame heap allocations.

use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::notestyle::{NoteStyleAssetData, NoteStyleData, NoteStyleDataCountdown};
use fnfrs_render::{
    BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextureHandle,
    SPRITE_FLAG_PIXEL_SNAP,
};

/// Discrete countdown phases matching `Countdown.CountdownStep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CountdownStep {
    #[default]
    Before,
    Three,
    Two,
    One,
    Go,
    Done,
}

/// Sound effect cue emitted when entering a countdown beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountdownSfx {
    Intro3,
    Intro2,
    Intro1,
    IntroGo,
}

impl CountdownSfx {
    /// Canonical cue key (`"intro3"`, `"intro2"`, `"intro1"`, `"introGo"`).
    #[must_use]
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::Intro3 => "intro3",
            Self::Intro2 => "intro2",
            Self::Intro1 => "intro1",
            Self::IntroGo => "introGo",
        }
    }

    /// V-Slice file suffix (`"introTHREE"`, `"introTWO"`, `"introONE"`, `"introGO"`).
    #[must_use]
    pub const fn vslice_name(self) -> &'static str {
        match self {
            Self::Intro3 => "introTHREE",
            Self::Intro2 => "introTWO",
            Self::Intro1 => "introONE",
            Self::IntroGo => "introGO",
        }
    }

    /// Zero-based index `[0..=3]` for SFX bank slot lookup.
    #[must_use]
    pub const fn as_index(self) -> usize {
        match self {
            Self::Intro3 => 0,
            Self::Intro2 => 1,
            Self::Intro1 => 2,
            Self::IntroGo => 3,
        }
    }
}

/// Loaded GPU texture and metadata for one countdown graphic (`ready`, `set`, `go`).
#[derive(Debug, Clone, Copy)]
pub struct CountdownGraphic {
    pub texture: TextureHandle,
    pub tex_size: [f32; 2],
    pub scale: f32,
    pub is_pixel: bool,
    pub offsets: [f32; 2],
}

/// Pre-loaded countdown graphics and audio paths for a `NoteStyle`.
#[derive(Debug, Clone, Default)]
pub struct CountdownAssets {
    /// Graphics corresponding to `[Two (ready), One (set), Go (go)]`.
    pub ready: Option<CountdownGraphic>,
    pub set: Option<CountdownGraphic>,
    pub go: Option<CountdownGraphic>,
    /// V-Slice audio keys `[introTHREE, introTWO, introONE, introGO]`.
    pub audio_keys: [Option<String>; 4],
    /// Optional registered `AudioEngine` SFX bank slots `[intro3, intro2, intro1, introGo]`.
    pub sfx_slots: [Option<usize>; 4],
    /// Whether the active style defaults to pixel-stepped fading.
    pub default_pixel: bool,
}

impl CountdownAssets {
    /// Creates an empty [`CountdownAssets`] (headless / geometric fallback).
    #[must_use]
    pub fn empty(is_pixel: bool) -> Self {
        let folder = if is_pixel { "pixel" } else { "funkin" };
        Self {
            ready: None,
            set: None,
            go: None,
            audio_keys: [
                Some(format!("shared:gameplay/countdown/{folder}/introTHREE")),
                Some(format!("shared:gameplay/countdown/{folder}/introTWO")),
                Some(format!("shared:gameplay/countdown/{folder}/introONE")),
                Some(format!("shared:gameplay/countdown/{folder}/introGO")),
            ],
            sfx_slots: [None; 4],
            default_pixel: is_pixel,
        }
    }

    /// Loads countdown graphics (`ready.png`, `set.png`, `go.png`) and audio keys from
    /// `data/notestyles/<style_id>.json`, walking `fallback` note styles if fields are omitted.
    pub fn load_from_assets(assets: &AssetLocator, gpu: &mut GpuContext, style_id: &str) -> Self {
        let mut chain = Vec::with_capacity(4);
        let mut current_id = Some(style_id.to_owned());
        for _ in 0..4 {
            let Some(id) = current_id.take() else {
                break;
            };
            let json_key = format!("notestyles/{id}");
            let json_path = assets.data_json(&json_key);
            let Ok(raw) = fs::read_to_string(&json_path) else {
                break;
            };
            let Ok(parsed) = serde_json::from_str::<NoteStyleData>(&raw) else {
                break;
            };
            current_id = parsed.fallback.clone();
            chain.push(parsed);
        }

        if chain.is_empty() {
            return Self::load_direct_fallback(assets, gpu, style_id.eq_ignore_ascii_case("pixel"));
        }

        let three_cfg = find_countdown_cfg(&chain, |d| d.assets.countdown_three.as_ref());
        let two_cfg = find_countdown_cfg(&chain, |d| d.assets.countdown_two.as_ref());
        let one_cfg = find_countdown_cfg(&chain, |d| d.assets.countdown_one.as_ref());
        let go_cfg = find_countdown_cfg(&chain, |d| d.assets.countdown_go.as_ref());

        let default_pixel = two_cfg.is_some_and(|c| c.is_pixel)
            || one_cfg.is_some_and(|c| c.is_pixel)
            || go_cfg.is_some_and(|c| c.is_pixel);

        let mut out = Self::empty(default_pixel);
        out.ready = load_graphic_from_cfg(assets, gpu, two_cfg);
        out.set = load_graphic_from_cfg(assets, gpu, one_cfg);
        out.go = load_graphic_from_cfg(assets, gpu, go_cfg);

        let cfgs = [three_cfg, two_cfg, one_cfg, go_cfg];
        for (i, cfg_opt) in cfgs.iter().enumerate() {
            if let Some(path) = cfg_opt
                .and_then(|c| c.data.as_ref())
                .map(|d| d.audio_path.clone())
                .filter(|p| !p.is_empty())
            {
                out.audio_keys[i] = Some(path);
            }
        }

        out
    }

    fn load_direct_fallback(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        is_pixel: bool,
    ) -> Self {
        let mut out = Self::empty(is_pixel);
        let folder = if is_pixel { "pixel" } else { "funkin" };
        let scale = if is_pixel { 6.0 } else { 1.0 };

        let ready_key = format!("shared:ui/countdown/{folder}/ready");
        let set_key = format!("shared:ui/countdown/{folder}/set");
        let go_key = format!("shared:ui/countdown/{folder}/go");

        out.ready = upload_graphic(assets, gpu, &ready_key, scale, is_pixel, [0.0, 0.0]);
        out.set = upload_graphic(assets, gpu, &set_key, scale, is_pixel, [0.0, 0.0]);
        out.go = upload_graphic(assets, gpu, &go_key, scale, is_pixel, [0.0, 0.0]);
        out
    }

    /// Returns the graphic for `step` (`Two -> ready`, `One -> set`, `Go -> go`).
    #[inline]
    #[must_use]
    pub const fn graphic_for(&self, step: CountdownStep) -> Option<&CountdownGraphic> {
        match step {
            CountdownStep::Two => self.ready.as_ref(),
            CountdownStep::One => self.set.as_ref(),
            CountdownStep::Go => self.go.as_ref(),
            CountdownStep::Before | CountdownStep::Three | CountdownStep::Done => None,
        }
    }

    /// Returns the resolved asset key for `sfx`, if available.
    #[inline]
    #[must_use]
    pub fn audio_key(&self, sfx: CountdownSfx) -> Option<&str> {
        self.audio_keys[sfx.as_index()].as_deref()
    }
}

fn find_countdown_cfg<'a, F>(
    chain: &'a [NoteStyleData],
    pick: F,
) -> Option<&'a NoteStyleAssetData<NoteStyleDataCountdown>>
where
    F: Fn(&'a NoteStyleData) -> Option<&'a NoteStyleAssetData<NoteStyleDataCountdown>>,
{
    chain.iter().find_map(pick)
}

fn load_graphic_from_cfg(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    cfg: Option<&NoteStyleAssetData<NoteStyleDataCountdown>>,
) -> Option<CountdownGraphic> {
    let cfg = cfg?;
    let key = cfg.asset_path.as_deref()?;
    upload_graphic(assets, gpu, key, cfg.scale, cfg.is_pixel, cfg.offsets)
}

fn upload_graphic(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
    scale: f32,
    is_pixel: bool,
    offsets: [f32; 2],
) -> Option<CountdownGraphic> {
    let path = assets.image(key);
    let bytes = fs::read(&path).ok()?;
    let texture = gpu.from_png_bytes(Some(key), &bytes).ok()?;
    let (w, h) = gpu.texture_size(texture)?;
    Some(CountdownGraphic {
        texture,
        tex_size: [w as f32, h as f32],
        scale: scale.max(0.05),
        is_pixel,
        offsets,
    })
}

/// Output emitted by [`CountdownController::update`] / [`CountdownController::update_to_time`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountdownTick {
    /// Current step after this update.
    pub step: CountdownStep,
    /// Newly entered step on this tick, if a beat boundary was crossed.
    pub entered_step: Option<CountdownStep>,
    /// SFX cue to trigger on this tick, if any.
    pub sfx: Option<CountdownSfx>,
    /// `true` on the frame the countdown reaches [`CountdownStep::Done`].
    pub finished_now: bool,
}

/// Zero-allocation V-Slice pre-song countdown controller (`Countdown.hx`).
///
/// Tracks negative pre-roll song time from `-4 * beat_length_ms` up to `start_timestamp_ms`
/// (`0.0` by default):
/// - `[-4 * beat, -3 * beat)`: `Three` + `intro3`
/// - `[-3 * beat, -2 * beat)`: `Two` + `intro2` (`ready.png`)
/// - `[-2 * beat, -1 * beat)`: `One` + `intro1` (`set.png`)
/// - `[-1 * beat, 0)`: `Go` + `introGo` (`go.png`)
/// - `>= 0`: `Done`
#[derive(Debug, Clone)]
pub struct CountdownController {
    pub step: CountdownStep,
    pub beat_length_ms: f64,
    pub start_timestamp_ms: f64,
    pub song_time_ms: f64,
    pub active: bool,
    pub is_paused: bool,
    step_elapsed_ms: f64,
    active_graphic_step: Option<CountdownStep>,
    graphic_alpha: f32,
}

impl Default for CountdownController {
    fn default() -> Self {
        Self::new(500.0)
    }
}

impl CountdownController {
    /// Creates a new idle [`CountdownController`] configured for `beat_length_ms`.
    #[must_use]
    pub fn new(beat_length_ms: f64) -> Self {
        let safe_beat = beat_length_ms.max(1.0);
        Self {
            step: CountdownStep::Before,
            beat_length_ms: safe_beat,
            start_timestamp_ms: 0.0,
            song_time_ms: -4.0 * safe_beat,
            active: false,
            is_paused: false,
            step_elapsed_ms: 0.0,
            active_graphic_step: None,
            graphic_alpha: 0.0,
        }
    }

    /// Starts (or restarts) the countdown from `-4 * beat_length_ms` down to `0.0`.
    pub fn start(&mut self, beat_length_ms: f64) {
        self.start_at(beat_length_ms, 0.0);
    }

    /// Starts (or restarts) the countdown ending at `start_timestamp_ms`.
    pub fn start_at(&mut self, beat_length_ms: f64, start_timestamp_ms: f64) {
        self.beat_length_ms = beat_length_ms.max(1.0);
        self.start_timestamp_ms = start_timestamp_ms;
        self.song_time_ms = start_timestamp_ms - 4.0 * self.beat_length_ms;
        self.step = CountdownStep::Before;
        self.active = true;
        self.is_paused = false;
        self.step_elapsed_ms = 0.0;
        self.active_graphic_step = None;
        self.graphic_alpha = 0.0;
    }

    /// Skips immediately to [`CountdownStep::Done`] at `start_timestamp_ms`.
    pub fn skip(&mut self) {
        self.step = CountdownStep::Done;
        self.song_time_ms = self.start_timestamp_ms;
        self.active = false;
        self.active_graphic_step = None;
        self.graphic_alpha = 0.0;
    }

    /// Pauses or resumes the countdown timer.
    pub fn set_paused(&mut self, paused: bool) {
        self.is_paused = paused;
    }

    /// Returns current alpha `[0.0, 1.0]` of the active countdown graphic (`ready`/`set`/`go`).
    #[inline]
    #[must_use]
    pub const fn graphic_alpha(&self) -> f32 {
        self.graphic_alpha
    }

    /// Returns which step's graphic is currently fading out, if any.
    #[inline]
    #[must_use]
    pub const fn active_graphic_step(&self) -> Option<CountdownStep> {
        self.active_graphic_step
    }

    /// Advances the countdown by `elapsed_s` seconds and evaluates beat ticks + fade tween
    /// with zero heap allocations.
    pub fn update(&mut self, elapsed_s: f32, is_pixel: bool) -> CountdownTick {
        if !self.active || self.is_paused {
            return CountdownTick {
                step: self.step,
                entered_step: None,
                sfx: None,
                finished_now: false,
            };
        }

        let dt_ms = f64::from(elapsed_s.max(0.0)) * 1000.0;
        let next_time = self.song_time_ms + dt_ms;
        self.update_to_time(next_time, dt_ms, is_pixel)
    }

    /// Synchronizes the countdown against an explicit negative-to-zero `song_time_ms`
    /// with zero heap allocations.
    pub fn update_to_time(
        &mut self,
        song_time_ms: f64,
        elapsed_ms: f64,
        is_pixel: bool,
    ) -> CountdownTick {
        if !self.active {
            return CountdownTick {
                step: self.step,
                entered_step: None,
                sfx: None,
                finished_now: false,
            };
        }

        self.song_time_ms = song_time_ms;
        let rel_ms = song_time_ms - self.start_timestamp_ms;
        let beat = self.beat_length_ms;

        let target_step = if rel_ms >= -1e-6 {
            CountdownStep::Done
        } else if rel_ms >= -beat - 1e-6 {
            CountdownStep::Go
        } else if rel_ms >= -2.0 * beat - 1e-6 {
            CountdownStep::One
        } else if rel_ms >= -3.0 * beat - 1e-6 {
            CountdownStep::Two
        } else {
            CountdownStep::Three
        };

        let mut entered_step = None;
        let mut sfx = None;
        let mut finished_now = false;

        if target_step != self.step {
            let next_step = match (self.step, target_step) {
                (CountdownStep::Before, _) => target_step,
                (_, target) => target,
            };
            self.step = next_step;
            entered_step = Some(next_step);
            self.step_elapsed_ms = 0.0;

            match next_step {
                CountdownStep::Three => {
                    sfx = Some(CountdownSfx::Intro3);
                    self.active_graphic_step = None;
                    self.graphic_alpha = 0.0;
                }
                CountdownStep::Two => {
                    sfx = Some(CountdownSfx::Intro2);
                    self.active_graphic_step = Some(CountdownStep::Two);
                    self.graphic_alpha = 1.0;
                }
                CountdownStep::One => {
                    sfx = Some(CountdownSfx::Intro1);
                    self.active_graphic_step = Some(CountdownStep::One);
                    self.graphic_alpha = 1.0;
                }
                CountdownStep::Go => {
                    sfx = Some(CountdownSfx::IntroGo);
                    self.active_graphic_step = Some(CountdownStep::Go);
                    self.graphic_alpha = 1.0;
                }
                CountdownStep::Done => {
                    self.active = false;
                    self.active_graphic_step = None;
                    self.graphic_alpha = 0.0;
                    finished_now = true;
                }
                CountdownStep::Before => {}
            }
        } else if self.active_graphic_step.is_some() {
            self.step_elapsed_ms += elapsed_ms.max(0.0);
        }

        if self.active_graphic_step.is_some() {
            let progress = (self.step_elapsed_ms / beat).clamp(0.0, 1.0) as f32;
            let eased = if is_pixel {
                ease_stepped(8, progress)
            } else {
                ease_cube_in_out(progress)
            };
            self.graphic_alpha = (1.0 - eased).clamp(0.0, 1.0);
        }

        CountdownTick {
            step: self.step,
            entered_step,
            sfx,
            finished_now,
        }
    }

    /// Queues the active countdown graphic (`ready.png`, `set.png`, `go.png`) centered
    /// on the `1280x720` virtual canvas with zero heap allocations.
    pub fn draw(&self, batcher: &mut SpriteBatcher, assets: Option<&CountdownAssets>) {
        let Some(step) = self.active_graphic_step else {
            return;
        };
        if self.graphic_alpha <= 0.001 {
            return;
        }
        let Some(graphic) = assets.and_then(|a| a.graphic_for(step)) else {
            return;
        };

        let draw_w = graphic.tex_size[0] * graphic.scale;
        let draw_h = graphic.tex_size[1] * graphic.scale;
        let draw_x = (VIRTUAL_WIDTH - draw_w) * 0.5 + graphic.offsets[0];
        let draw_y = (VIRTUAL_HEIGHT - draw_h) * 0.5 + graphic.offsets[1];

        let filter = if graphic.is_pixel {
            FilterMode::Nearest
        } else {
            FilterMode::Linear
        };
        let flags = if graphic.is_pixel {
            SPRITE_FLAG_PIXEL_SNAP
        } else {
            0
        };

        batcher.push(
            graphic.texture,
            BlendMode::Normal,
            filter,
            SpriteInstance::new([draw_x, draw_y], [draw_w, draw_h])
                .with_color([1.0, 1.0, 1.0, self.graphic_alpha])
                .with_flags(flags),
        );
    }
}

/// HaxeFlixel `FlxEase.cubeInOut` (`t <= 0.5 ? 4*t^3 : 1 + 4*(t-1)^3`).
#[inline]
#[must_use]
pub fn ease_cube_in_out(t: f32) -> f32 {
    let clamped = t.clamp(0.0, 1.0);
    if clamped <= 0.5 {
        4.0 * clamped * clamped * clamped
    } else {
        let u = clamped - 1.0;
        1.0 + 4.0 * u * u * u
    }
}

/// V-Slice `EaseUtil.stepped(steps)` (`floor(t * steps) / steps`).
#[inline]
#[must_use]
pub fn ease_stepped(steps: u32, t: f32) -> f32 {
    if steps == 0 {
        return t.clamp(0.0, 1.0);
    }
    let s = steps as f32;
    (t.clamp(0.0, 1.0) * s).floor() / s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_ticks_on_exact_beats_across_bpms() {
        for bpm in [60.0_f64, 120.0, 180.0] {
            let beat_ms = 60_000.0 / bpm;
            let mut ctrl = CountdownController::new(beat_ms);
            ctrl.start(beat_ms);
            assert!((ctrl.song_time_ms - (-4.0 * beat_ms)).abs() < 1e-6);

            // Initial frame at -4 * beat_ms => Three + intro3
            let t0 = ctrl.update(0.0, false);
            assert_eq!(t0.step, CountdownStep::Three);
            assert_eq!(t0.entered_step, Some(CountdownStep::Three));
            assert_eq!(t0.sfx, Some(CountdownSfx::Intro3));
            assert_eq!(t0.sfx.map(CountdownSfx::as_key), Some("intro3"));
            assert!(!t0.finished_now);

            // Half beat later => still Three, no new SFX
            let mid = ctrl.update((beat_ms * 0.5 / 1000.0) as f32, false);
            assert_eq!(mid.step, CountdownStep::Three);
            assert_eq!(mid.entered_step, None);
            assert_eq!(mid.sfx, None);

            // Beat 1 (-3 * beat_ms) => Two (ready) + intro2
            let t1 = ctrl.update((beat_ms * 0.5 / 1000.0) as f32, false);
            assert_eq!(t1.step, CountdownStep::Two);
            assert_eq!(t1.entered_step, Some(CountdownStep::Two));
            assert_eq!(t1.sfx, Some(CountdownSfx::Intro2));
            assert_eq!(t1.sfx.map(CountdownSfx::as_key), Some("intro2"));
            assert!((ctrl.graphic_alpha() - 1.0).abs() < 1e-4);

            // Beat 2 (-2 * beat_ms) => One (set) + intro1
            let t2 = ctrl.update((beat_ms / 1000.0) as f32, false);
            assert_eq!(t2.step, CountdownStep::One);
            assert_eq!(t2.entered_step, Some(CountdownStep::One));
            assert_eq!(t2.sfx, Some(CountdownSfx::Intro1));
            assert_eq!(t2.sfx.map(CountdownSfx::as_key), Some("intro1"));

            // Beat 3 (-1 * beat_ms) => Go + introGo
            let t3 = ctrl.update((beat_ms / 1000.0) as f32, false);
            assert_eq!(t3.step, CountdownStep::Go);
            assert_eq!(t3.entered_step, Some(CountdownStep::Go));
            assert_eq!(t3.sfx, Some(CountdownSfx::IntroGo));
            assert_eq!(t3.sfx.map(CountdownSfx::as_key), Some("introGo"));

            // Halfway through Go beat: FlxEase.cubeInOut(0.5) == 0.5 => alpha == 0.5
            let go_mid = ctrl.update((beat_ms * 0.5 / 1000.0) as f32, false);
            assert_eq!(go_mid.step, CountdownStep::Go);
            assert!((ctrl.graphic_alpha() - 0.5).abs() < 1e-3);

            // Beat 4 (0.0 ms) => Done
            let t4 = ctrl.update((beat_ms * 0.5 / 1000.0) as f32, false);
            assert_eq!(t4.step, CountdownStep::Done);
            assert_eq!(t4.entered_step, Some(CountdownStep::Done));
            assert_eq!(t4.sfx, None);
            assert!(t4.finished_now);
            assert!(!ctrl.active);
        }
    }

    #[test]
    fn cube_in_out_and_stepped_easing_match_vslice() {
        assert!((ease_cube_in_out(0.0) - 0.0).abs() < 1e-6);
        assert!((ease_cube_in_out(0.25) - 0.0625).abs() < 1e-6);
        assert!((ease_cube_in_out(0.5) - 0.5).abs() < 1e-6);
        assert!((ease_cube_in_out(0.75) - 0.9375).abs() < 1e-6);
        assert!((ease_cube_in_out(1.0) - 1.0).abs() < 1e-6);

        assert!((ease_stepped(8, 0.0) - 0.0).abs() < 1e-6);
        assert!((ease_stepped(8, 0.124) - 0.0).abs() < 1e-6);
        assert!((ease_stepped(8, 0.125) - 0.125).abs() < 1e-6);
        assert!((ease_stepped(8, 0.999) - 0.875).abs() < 1e-6);
        assert!((ease_stepped(8, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pixel_countdown_uses_stepped_fade_and_pause_freezes_progress() {
        let mut ctrl = CountdownController::new(400.0);
        ctrl.start(400.0);
        let _ = ctrl.update(0.0, true); // Three
        let _ = ctrl.update(0.4, true); // Two (ready)
        assert_eq!(ctrl.step, CountdownStep::Two);
        assert!((ctrl.graphic_alpha() - 1.0).abs() < 1e-6);

        // 0.04s / 0.4s = 0.1 -> stepped(8, 0.1) = 0.0 -> alpha stays 1.0
        let _ = ctrl.update(0.04, true);
        assert!((ctrl.graphic_alpha() - 1.0).abs() < 1e-6);

        // Pause freezes timer and graphic fade
        ctrl.set_paused(true);
        let paused_tick = ctrl.update(0.2, true);
        assert_eq!(paused_tick.step, CountdownStep::Two);
        assert!((ctrl.graphic_alpha() - 1.0).abs() < 1e-6);

        ctrl.set_paused(false);
        // Total elapsed 0.1s -> t = 0.25 -> stepped(8, 0.25) = 0.25 -> alpha = 0.75
        let _ = ctrl.update(0.06, true);
        assert!((ctrl.graphic_alpha() - 0.75).abs() < 1e-6);
    }
}
