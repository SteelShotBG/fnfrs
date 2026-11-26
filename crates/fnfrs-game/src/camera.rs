//! 2D gameplay/HUD camera (`FunkinCamera`, `FlxCamera`) and V-Slice easing
//! functions (`FlxEase.hx`) with zero per-frame heap allocations.

use std::f32::consts::{PI, TAU};

use fnfrs_core::constants::{
    DEFAULT_BOP_INTENSITY, DEFAULT_CAMERA_FOLLOW_RATE, DEFAULT_ZOOM_RATE, MS_PER_SEC,
    STEPS_PER_BEAT, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};

/// Fallback stage zoom when no stage metadata is loaded (`PlayState.stageZoom`).
pub const DEFAULT_STAGE_ZOOM: f32 = 1.05;

/// Per-frame (at 60 FPS) exponential decay base for camera bops (`PlayState.hx`).
pub const CAMERA_BOP_DECAY_RATE: f32 = 0.95;

/// Maximum HUD/game zoom ratio allowed before skipping beat bops (`PlayState.hx`).
pub const MAX_RELATIVE_CAM_ZOOM: f32 = 1.35;

/// Default HUD camera bop offset added per bop (`0.015 * 2.0`).
pub const DEFAULT_HUD_BOP_INTENSITY: f32 = 0.015 * 2.0;

const ELASTIC_PERIOD: f32 = 0.4;
const BACK_OVERSHOOT: f32 = 1.70158;
const BACK_INOUT_OVERSHOOT: f32 = BACK_OVERSHOOT * 1.525;

const BOUNCE_B1: f32 = 1.0 / 2.75;
const BOUNCE_B2: f32 = 2.0 / 2.75;
const BOUNCE_B3: f32 = 1.5 / 2.75;
const BOUNCE_B4: f32 = 2.5 / 2.75;
const BOUNCE_B5: f32 = 2.25 / 2.75;
const BOUNCE_B6: f32 = 2.625 / 2.75;

/// Easing curves matching `flixel.tweens.FlxEase` and V-Slice song event schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlxEase {
    #[default]
    Linear,
    Instant,
    QuadIn,
    QuadOut,
    QuadInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    QuartIn,
    QuartOut,
    QuartInOut,
    QuintIn,
    QuintOut,
    QuintInOut,
    SineIn,
    SineOut,
    SineInOut,
    ExpoIn,
    ExpoOut,
    ExpoInOut,
    CircIn,
    CircOut,
    CircInOut,
    ElasticIn,
    ElasticOut,
    ElasticInOut,
    BackIn,
    BackOut,
    BackInOut,
    BounceIn,
    BounceOut,
    BounceInOut,
    SmoothStepIn,
    SmoothStepOut,
    SmoothStepInOut,
    SmootherStepIn,
    SmootherStepOut,
    SmootherStepInOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EaseDir {
    In,
    Out,
    InOut,
}

impl EaseDir {
    #[inline]
    fn parse(dir: &str) -> Option<Self> {
        if dir.eq_ignore_ascii_case("in") {
            Some(Self::In)
        } else if dir.eq_ignore_ascii_case("out") {
            Some(Self::Out)
        } else if dir.eq_ignore_ascii_case("inout") {
            Some(Self::InOut)
        } else {
            None
        }
    }
}

impl FlxEase {
    /// Parses a V-Slice / FlxEase easing name case-insensitively with zero heap allocations.
    ///
    /// Supports both `"cubicIn"` and Flixel's `"cubeIn"` naming, plus `"linear"`,
    /// `"INSTANT"`, and `"smootherStepIn/Out/InOut"`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("linear")
            || name.eq_ignore_ascii_case("linearIn")
            || name.eq_ignore_ascii_case("linearOut")
            || name.eq_ignore_ascii_case("linearInOut")
        {
            return Some(Self::Linear);
        }
        if name.eq_ignore_ascii_case("instant") {
            return Some(Self::Instant);
        }

        // Split trailing direction suffix without allocating.
        let (base, dir) = if let Some(prefix) = strip_suffix_ignore_ascii_case(name, "inout") {
            (prefix, EaseDir::InOut)
        } else if let Some(prefix) = strip_suffix_ignore_ascii_case(name, "out") {
            (prefix, EaseDir::Out)
        } else {
            let prefix = strip_suffix_ignore_ascii_case(name, "in")?;
            (prefix, EaseDir::In)
        };

        from_base_and_dir(base, dir)
    }

    /// Parses a V-Slice `(ease, easeDir)` pair with zero heap allocations.
    ///
    /// Mirrors `SongEvent.hx`: if `ease` is `"linear"`, `"INSTANT"`, or already ends with
    /// `In`/`Out`/`InOut`, `ease_dir` is ignored. Otherwise `ease_dir` defaults to `"In"`.
    #[must_use]
    pub fn parse_with_dir(ease: &str, ease_dir: Option<&str>) -> Option<Self> {
        let ease = ease.trim();
        if let Some(parsed) = Self::parse(ease) {
            return Some(parsed);
        }
        let dir = match ease_dir {
            Some(d) => EaseDir::parse(d.trim())?,
            None => EaseDir::In,
        };
        from_base_and_dir(ease, dir)
    }

    /// Evaluates the easing curve at normalized progress `t` (`0.0..=1.0`).
    #[must_use]
    pub fn eval(self, t: f32) -> f32 {
        if self == Self::Instant {
            return 1.0;
        }
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Instant => 1.0,

            Self::QuadIn => t * t,
            Self::QuadOut => -t * (t - 2.0),
            Self::QuadInOut => {
                if t <= 0.5 {
                    t * t * 2.0
                } else {
                    let u = t - 1.0;
                    1.0 - u * u * 2.0
                }
            }

            Self::CubicIn => t * t * t,
            Self::CubicOut => {
                let u = t - 1.0;
                1.0 + u * u * u
            }
            Self::CubicInOut => {
                if t <= 0.5 {
                    t * t * t * 4.0
                } else {
                    let u = t - 1.0;
                    1.0 + u * u * u * 4.0
                }
            }

            Self::QuartIn => t * t * t * t,
            Self::QuartOut => {
                let u = t - 1.0;
                1.0 - u * u * u * u
            }
            Self::QuartInOut => {
                if t <= 0.5 {
                    t * t * t * t * 8.0
                } else {
                    let u = t * 2.0 - 2.0;
                    (1.0 - u * u * u * u) * 0.5 + 0.5
                }
            }

            Self::QuintIn => t * t * t * t * t,
            Self::QuintOut => {
                let u = t - 1.0;
                u * u * u * u * u + 1.0
            }
            Self::QuintInOut => {
                let mut u = t * 2.0;
                if u < 1.0 {
                    (u * u * u * u * u) * 0.5
                } else {
                    u -= 2.0;
                    (u * u * u * u * u + 2.0) * 0.5
                }
            }

            Self::SineIn => -(PI * 0.5 * t).cos() + 1.0,
            Self::SineOut => (PI * 0.5 * t).sin(),
            Self::SineInOut => -(PI * t).cos() * 0.5 + 0.5,

            Self::ExpoIn => {
                if t <= 0.0 {
                    0.0
                } else {
                    2.0_f32.powf(10.0 * (t - 1.0))
                }
            }
            Self::ExpoOut => {
                if t >= 1.0 {
                    1.0
                } else if t <= 0.0 {
                    0.0
                } else {
                    -2.0_f32.powf(-10.0 * t) + 1.0
                }
            }
            Self::ExpoInOut => {
                if t <= 0.0 {
                    0.0
                } else if t >= 1.0 {
                    1.0
                } else if t < 0.5 {
                    0.5 * 2.0_f32.powf(20.0 * t - 10.0)
                } else {
                    1.0 - 0.5 * 2.0_f32.powf(-20.0 * t + 10.0)
                }
            }

            Self::CircIn => -((1.0 - t * t).max(0.0).sqrt() - 1.0),
            Self::CircOut => {
                let u = t - 1.0;
                (1.0 - u * u).max(0.0).sqrt()
            }
            Self::CircInOut => {
                if t <= 0.5 {
                    let u = t * 2.0;
                    (1.0 - (1.0 - u * u).max(0.0).sqrt()) * 0.5
                } else {
                    let u = t * 2.0 - 2.0;
                    ((1.0 - u * u).max(0.0).sqrt() + 1.0) * 0.5
                }
            }

            Self::ElasticIn => {
                if t <= 0.0 {
                    0.0
                } else if t >= 1.0 {
                    1.0
                } else {
                    let u = t - 1.0;
                    -(2.0_f32.powf(10.0 * u)
                        * ((u - ELASTIC_PERIOD * 0.25) * TAU / ELASTIC_PERIOD).sin())
                }
            }
            Self::ElasticOut => {
                if t <= 0.0 {
                    0.0
                } else if t >= 1.0 {
                    1.0
                } else {
                    2.0_f32.powf(-10.0 * t)
                        * ((t - ELASTIC_PERIOD * 0.25) * TAU / ELASTIC_PERIOD).sin()
                        + 1.0
                }
            }
            Self::ElasticInOut => {
                if t <= 0.0 {
                    0.0
                } else if t >= 1.0 {
                    1.0
                } else {
                    let u = 2.0 * t - 1.0;
                    let s = ELASTIC_PERIOD * 0.25;
                    if u < 0.0 {
                        -0.5 * 2.0_f32.powf(10.0 * u) * ((u - s) * TAU / ELASTIC_PERIOD).sin()
                    } else {
                        0.5 * 2.0_f32.powf(-10.0 * u) * ((u - s) * TAU / ELASTIC_PERIOD).sin()
                            + 1.0
                    }
                }
            }

            Self::BackIn => t * t * ((BACK_OVERSHOOT + 1.0) * t - BACK_OVERSHOOT),
            Self::BackOut => {
                let u = t - 1.0;
                u * u * ((BACK_OVERSHOOT + 1.0) * u + BACK_OVERSHOOT) + 1.0
            }
            Self::BackInOut => {
                let mut u = t * 2.0;
                if u < 1.0 {
                    0.5 * (u * u * ((BACK_INOUT_OVERSHOOT + 1.0) * u - BACK_INOUT_OVERSHOOT))
                } else {
                    u -= 2.0;
                    0.5 * (u * u * ((BACK_INOUT_OVERSHOOT + 1.0) * u + BACK_INOUT_OVERSHOOT) + 2.0)
                }
            }

            Self::BounceIn => 1.0 - bounce_out(1.0 - t),
            Self::BounceOut => bounce_out(t),
            Self::BounceInOut => {
                if t < 0.5 {
                    0.5 * (1.0 - bounce_out(1.0 - t * 2.0))
                } else {
                    0.5 * bounce_out(t * 2.0 - 1.0) + 0.5
                }
            }

            Self::SmoothStepIn => 2.0 * smooth_step(t * 0.5),
            Self::SmoothStepOut => 2.0 * smooth_step(t * 0.5 + 0.5) - 1.0,
            Self::SmoothStepInOut => smooth_step(t),

            Self::SmootherStepIn => 2.0 * smoother_step(t * 0.5),
            Self::SmootherStepOut => 2.0 * smoother_step(t * 0.5 + 0.5) - 1.0,
            Self::SmootherStepInOut => smoother_step(t),
        }
    }
}

#[inline]
fn bounce_out(t: f32) -> f32 {
    if t < BOUNCE_B1 {
        7.5625 * t * t
    } else if t < BOUNCE_B2 {
        let u = t - BOUNCE_B3;
        7.5625 * u * u + 0.75
    } else if t < BOUNCE_B4 {
        let u = t - BOUNCE_B5;
        7.5625 * u * u + 0.9375
    } else {
        let u = t - BOUNCE_B6;
        7.5625 * u * u + 0.984375
    }
}

#[inline]
fn smooth_step(t: f32) -> f32 {
    t * t * (t * -2.0 + 3.0)
}

#[inline]
fn smoother_step(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn strip_suffix_ignore_ascii_case<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let s_bytes = s.as_bytes();
    let suf_bytes = suffix.as_bytes();
    if s_bytes.len() < suf_bytes.len() {
        return None;
    }
    let split = s_bytes.len() - suf_bytes.len();
    if s_bytes[split..].eq_ignore_ascii_case(suf_bytes) {
        Some(&s[..split])
    } else {
        None
    }
}

fn from_base_and_dir(base: &str, dir: EaseDir) -> Option<FlxEase> {
    if base.eq_ignore_ascii_case("quad") {
        Some(match dir {
            EaseDir::In => FlxEase::QuadIn,
            EaseDir::Out => FlxEase::QuadOut,
            EaseDir::InOut => FlxEase::QuadInOut,
        })
    } else if base.eq_ignore_ascii_case("cubic") || base.eq_ignore_ascii_case("cube") {
        Some(match dir {
            EaseDir::In => FlxEase::CubicIn,
            EaseDir::Out => FlxEase::CubicOut,
            EaseDir::InOut => FlxEase::CubicInOut,
        })
    } else if base.eq_ignore_ascii_case("quart") {
        Some(match dir {
            EaseDir::In => FlxEase::QuartIn,
            EaseDir::Out => FlxEase::QuartOut,
            EaseDir::InOut => FlxEase::QuartInOut,
        })
    } else if base.eq_ignore_ascii_case("quint") {
        Some(match dir {
            EaseDir::In => FlxEase::QuintIn,
            EaseDir::Out => FlxEase::QuintOut,
            EaseDir::InOut => FlxEase::QuintInOut,
        })
    } else if base.eq_ignore_ascii_case("sine") {
        Some(match dir {
            EaseDir::In => FlxEase::SineIn,
            EaseDir::Out => FlxEase::SineOut,
            EaseDir::InOut => FlxEase::SineInOut,
        })
    } else if base.eq_ignore_ascii_case("expo") {
        Some(match dir {
            EaseDir::In => FlxEase::ExpoIn,
            EaseDir::Out => FlxEase::ExpoOut,
            EaseDir::InOut => FlxEase::ExpoInOut,
        })
    } else if base.eq_ignore_ascii_case("circ") {
        Some(match dir {
            EaseDir::In => FlxEase::CircIn,
            EaseDir::Out => FlxEase::CircOut,
            EaseDir::InOut => FlxEase::CircInOut,
        })
    } else if base.eq_ignore_ascii_case("elastic") {
        Some(match dir {
            EaseDir::In => FlxEase::ElasticIn,
            EaseDir::Out => FlxEase::ElasticOut,
            EaseDir::InOut => FlxEase::ElasticInOut,
        })
    } else if base.eq_ignore_ascii_case("back") {
        Some(match dir {
            EaseDir::In => FlxEase::BackIn,
            EaseDir::Out => FlxEase::BackOut,
            EaseDir::InOut => FlxEase::BackInOut,
        })
    } else if base.eq_ignore_ascii_case("bounce") {
        Some(match dir {
            EaseDir::In => FlxEase::BounceIn,
            EaseDir::Out => FlxEase::BounceOut,
            EaseDir::InOut => FlxEase::BounceInOut,
        })
    } else if base.eq_ignore_ascii_case("smoothstep") {
        Some(match dir {
            EaseDir::In => FlxEase::SmoothStepIn,
            EaseDir::Out => FlxEase::SmoothStepOut,
            EaseDir::InOut => FlxEase::SmoothStepInOut,
        })
    } else if base.eq_ignore_ascii_case("smootherstep") {
        Some(match dir {
            EaseDir::In => FlxEase::SmootherStepIn,
            EaseDir::Out => FlxEase::SmootherStepOut,
            EaseDir::InOut => FlxEase::SmootherStepInOut,
        })
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PanTween {
    active: bool,
    start: [f32; 2],
    target: [f32; 2],
    elapsed_s: f32,
    duration_s: f32,
    ease: FlxEase,
}

#[derive(Debug, Clone, Copy, Default)]
struct ZoomTween {
    active: bool,
    start: f32,
    target: f32,
    elapsed_s: f32,
    duration_s: f32,
    ease: FlxEase,
}

/// 2D virtual camera (`camGame` / `camHUD`) matching `FlxCamera` + `PlayState.hx`
/// on the `1280 x 720` virtual canvas. Zero per-frame heap allocations.
#[derive(Debug, Clone)]
pub struct Camera2D {
    pub follow_point: [f32; 2],
    pub target_follow: [f32; 2],
    pub follow_lerp: f32,
    pub scroll: [f32; 2],
    pub zoom: f32,
    pub default_zoom: f32,
    pub bop_multiplier: f32,
    pub hud_bop_offset: f32,
    pub alpha: f32,
    pub visible: bool,

    pub shake_intensity: f32,
    pub shake_duration_s: f32,
    pub shake_offset: [f32; 2],

    pub flash_color: [f32; 3],
    pub flash_alpha: f32,
    flash_start_alpha: f32,
    flash_duration_s: f32,
    flash_elapsed_s: f32,

    pan_tween: PanTween,
    zoom_tween: ZoomTween,
    rng_state: u32,
}

impl Default for Camera2D {
    fn default() -> Self {
        Self::new_game(1.0)
    }
}

impl Camera2D {
    /// Creates a world-space gameplay camera (`camGame`) centered on `(640, 360)`.
    #[must_use]
    pub fn new_game(default_zoom: f32) -> Self {
        let center = [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5];
        Self {
            follow_point: center,
            target_follow: center,
            follow_lerp: DEFAULT_CAMERA_FOLLOW_RATE,
            scroll: [0.0, 0.0],
            zoom: default_zoom,
            default_zoom,
            bop_multiplier: 1.0,
            hud_bop_offset: 0.0,
            alpha: 1.0,
            visible: true,
            shake_intensity: 0.0,
            shake_duration_s: 0.0,
            shake_offset: [0.0, 0.0],
            flash_color: [1.0, 1.0, 1.0],
            flash_alpha: 0.0,
            flash_start_alpha: 0.0,
            flash_duration_s: 0.0,
            flash_elapsed_s: 0.0,
            pan_tween: PanTween::default(),
            zoom_tween: ZoomTween::default(),
            rng_state: 0x5A17_9C21,
        }
    }

    /// Creates an overlay HUD camera (`camHUD`) with fixed scroll `[0, 0]` and `1.0` zoom.
    #[must_use]
    pub fn new_hud() -> Self {
        let mut cam = Self::new_game(1.0);
        cam.follow_lerp = 1.0;
        cam
    }

    /// Effective zoom including beat bop multiplier (`camGame`) and HUD bop offset (`camHUD`).
    #[inline]
    #[must_use]
    pub fn effective_zoom(&self) -> f32 {
        (self.zoom * self.bop_multiplier + self.hud_bop_offset).max(0.01)
    }

    /// Returns `true` if a `FocusCamera` pan tween is currently running.
    #[inline]
    #[must_use]
    pub const fn is_pan_tweening(&self) -> bool {
        self.pan_tween.active
    }

    /// Returns `true` if a `ZoomCamera` zoom tween is currently running.
    #[inline]
    #[must_use]
    pub const fn is_zoom_tweening(&self) -> bool {
        self.zoom_tween.active
    }

    /// Sets the follow target for classic lerp tracking without snapping or starting a tween.
    pub fn set_target(&mut self, x: f32, y: f32) {
        self.pan_tween.active = false;
        self.target_follow = [x, y];
    }

    /// Immediately snaps `follow_point`, `target_follow`, and `scroll` to `(x, y)`.
    pub fn snap_to(&mut self, x: f32, y: f32) {
        self.pan_tween.active = false;
        self.target_follow = [x, y];
        self.follow_point = [x, y];
        self.sync_scroll_from_follow();
    }

    /// Resets camera zoom, active tweens, and/or snap state matching `PlayState.resetCamera`.
    pub fn reset(&mut self, reset_zoom: bool, cancel_tweens: bool, snap: bool) {
        if cancel_tweens {
            self.cancel_all_tweens();
        }
        if reset_zoom {
            self.zoom = self.default_zoom;
            self.bop_multiplier = 1.0;
            self.hud_bop_offset = 0.0;
        }
        if snap {
            self.follow_point = self.target_follow;
            self.sync_scroll_from_follow();
        }
    }

    /// Pans the camera to `(x, y)` over `duration_s` seconds using `ease`.
    ///
    /// A `duration_s <= 0.0` or [`FlxEase::Instant`] snaps immediately.
    pub fn tween_to(&mut self, x: f32, y: f32, duration_s: f32, ease: FlxEase) {
        self.target_follow = [x, y];
        if duration_s <= 0.0 || ease == FlxEase::Instant {
            self.snap_to(x, y);
            return;
        }
        self.pan_tween = PanTween {
            active: true,
            start: self.follow_point,
            target: [x, y],
            elapsed_s: 0.0,
            duration_s,
            ease,
        };
    }

    /// Pans the camera to `(x, y)` over `duration_steps` musical steps.
    pub fn tween_to_steps(
        &mut self,
        x: f32,
        y: f32,
        duration_steps: f32,
        step_length_ms: f64,
        ease: FlxEase,
    ) {
        let duration_s = ((f64::from(duration_steps) * step_length_ms) / MS_PER_SEC) as f32;
        self.tween_to(x, y, duration_s, ease);
    }

    /// Cancels any active pan tween.
    pub fn cancel_pan_tween(&mut self) {
        self.pan_tween.active = false;
    }

    /// Tweens camera `zoom` toward `target_zoom` over `duration_s` seconds using `ease`.
    ///
    /// A `duration_s <= 0.0` or [`FlxEase::Instant`] applies immediately.
    pub fn tween_zoom(&mut self, target_zoom: f32, duration_s: f32, ease: FlxEase) {
        if duration_s <= 0.0 || ease == FlxEase::Instant {
            self.zoom_tween.active = false;
            self.zoom = target_zoom;
            return;
        }
        self.zoom_tween = ZoomTween {
            active: true,
            start: self.zoom,
            target: target_zoom,
            elapsed_s: 0.0,
            duration_s,
            ease,
        };
    }

    /// Tweens camera `zoom` using V-Slice `ZoomCamera` step duration and `direct`/`stage` mode.
    pub fn tween_zoom_steps(
        &mut self,
        zoom: f32,
        direct: bool,
        duration_steps: f32,
        step_length_ms: f64,
        ease: FlxEase,
    ) {
        let target_zoom = if direct { zoom } else { zoom * self.default_zoom };
        let duration_s = ((f64::from(duration_steps) * step_length_ms) / MS_PER_SEC) as f32;
        self.tween_zoom(target_zoom, duration_s, ease);
    }

    /// Cancels any active zoom tween.
    pub fn cancel_zoom_tween(&mut self) {
        self.zoom_tween.active = false;
    }

    /// Cancels both pan and zoom tweens.
    pub fn cancel_all_tweens(&mut self) {
        self.pan_tween.active = false;
        self.zoom_tween.active = false;
    }

    /// Applies a gameplay camera zoom bop multiplier (e.g. `1.015`),
    /// clamped under `1.35 * default_zoom`.
    pub fn bop(&mut self, intensity: f32) {
        if self.effective_zoom() < MAX_RELATIVE_CAM_ZOOM * self.default_zoom {
            self.bop_multiplier = intensity;
        }
    }

    /// Applies a HUD camera zoom bop offset (e.g. `0.03`), clamped under `1.35 * default_zoom`.
    pub fn bop_hud(&mut self, hud_intensity: f32) {
        if self.effective_zoom() < MAX_RELATIVE_CAM_ZOOM * self.default_zoom {
            self.hud_bop_offset += hud_intensity * self.default_zoom;
        }
    }

    /// Starts a camera shake with normalized `intensity` (fraction of virtual canvas)
    /// for `duration_s` seconds.
    pub fn shake(&mut self, intensity: f32, duration_s: f32) {
        if intensity <= 0.0 || duration_s <= 0.0 {
            self.stop_shake();
            return;
        }
        self.shake_intensity = intensity;
        self.shake_duration_s = duration_s;
    }

    /// Stops any active camera shake immediately.
    pub fn stop_shake(&mut self) {
        self.shake_intensity = 0.0;
        self.shake_duration_s = 0.0;
        self.shake_offset = [0.0, 0.0];
    }

    /// Triggers a full-screen camera flash fading linearly from `color[3]` to `0.0`
    /// over `duration_s` seconds.
    pub fn flash(&mut self, color: [f32; 4], duration_s: f32) {
        if duration_s <= 0.0 || color[3] <= 0.0 {
            self.flash_alpha = 0.0;
            self.flash_duration_s = 0.0;
            return;
        }
        self.flash_color = [color[0], color[1], color[2]];
        self.flash_start_alpha = color[3].clamp(0.0, 1.0);
        self.flash_alpha = self.flash_start_alpha;
        self.flash_duration_s = duration_s;
        self.flash_elapsed_s = 0.0;
    }

    /// Returns the active flash overlay RGBA if visible.
    #[must_use]
    pub fn flash_overlay(&self) -> Option<[f32; 4]> {
        if self.flash_alpha > 0.0 {
            Some([
                self.flash_color[0],
                self.flash_color[1],
                self.flash_color[2],
                self.flash_alpha,
            ])
        } else {
            None
        }
    }

    /// Advances tweens, follow lerp, bop decay, shake, and flash with **zero heap allocations**.
    pub fn update(&mut self, elapsed_s: f32) {
        let dt = elapsed_s.max(0.0);
        if dt == 0.0 {
            return;
        }

        // 1. Pan tween or frame-rate-independent follow lerp.
        if self.pan_tween.active {
            self.pan_tween.elapsed_s += dt;
            let raw_t = if self.pan_tween.duration_s > 0.0 {
                (self.pan_tween.elapsed_s / self.pan_tween.duration_s).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let eased = self.pan_tween.ease.eval(raw_t);
            self.follow_point[0] = self.pan_tween.start[0]
                + (self.pan_tween.target[0] - self.pan_tween.start[0]) * eased;
            self.follow_point[1] = self.pan_tween.start[1]
                + (self.pan_tween.target[1] - self.pan_tween.start[1]) * eased;
            if raw_t >= 1.0 {
                self.follow_point = self.pan_tween.target;
                self.pan_tween.active = false;
            }
        } else {
            let rate = self.follow_lerp.clamp(0.0, 1.0);
            let alpha = if rate >= 1.0 {
                1.0
            } else {
                1.0 - (1.0 - rate).powf(dt * 60.0)
            };
            self.follow_point[0] += (self.target_follow[0] - self.follow_point[0]) * alpha;
            self.follow_point[1] += (self.target_follow[1] - self.follow_point[1]) * alpha;
        }
        self.sync_scroll_from_follow();

        // 2. Zoom tween.
        if self.zoom_tween.active {
            self.zoom_tween.elapsed_s += dt;
            let raw_t = if self.zoom_tween.duration_s > 0.0 {
                (self.zoom_tween.elapsed_s / self.zoom_tween.duration_s).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let eased = self.zoom_tween.ease.eval(raw_t);
            self.zoom = self.zoom_tween.start
                + (self.zoom_tween.target - self.zoom_tween.start) * eased;
            if raw_t >= 1.0 {
                self.zoom = self.zoom_tween.target;
                self.zoom_tween.active = false;
            }
        }

        // 3. Bop decay (`PlayState.hx`: `FlxMath.lerp(1.0, mult, 0.95^(dt*60))`).
        let decay = CAMERA_BOP_DECAY_RATE.powf(dt * 60.0);
        self.bop_multiplier = 1.0 + (self.bop_multiplier - 1.0) * decay;
        self.hud_bop_offset *= decay;
        if (self.bop_multiplier - 1.0).abs() < 1e-6 {
            self.bop_multiplier = 1.0;
        }
        if self.hud_bop_offset.abs() < 1e-6 {
            self.hud_bop_offset = 0.0;
        }

        // 4. Camera shake.
        if self.shake_duration_s > 0.0 {
            self.shake_duration_s -= dt;
            if self.shake_duration_s <= 0.0 {
                self.stop_shake();
            } else {
                let rx = self.next_signed_unit();
                let ry = self.next_signed_unit();
                self.shake_offset = [
                    rx * self.shake_intensity * VIRTUAL_WIDTH,
                    ry * self.shake_intensity * VIRTUAL_HEIGHT,
                ];
            }
        }

        // 5. Camera flash fade.
        if self.flash_alpha > 0.0 && self.flash_duration_s > 0.0 {
            self.flash_elapsed_s += dt;
            let progress = (self.flash_elapsed_s / self.flash_duration_s).clamp(0.0, 1.0);
            self.flash_alpha = self.flash_start_alpha * (1.0 - progress);
            if progress >= 1.0 {
                self.flash_alpha = 0.0;
                self.flash_duration_s = 0.0;
            }
        }
    }

    /// Transforms a world-space rectangle `(pos, size)` with `scroll_factor` into
    /// `1280 x 720` virtual screen coordinates with centered camera zoom.
    #[must_use]
    pub fn world_to_screen(
        &self,
        pos: [f32; 2],
        size: [f32; 2],
        scroll_factor: [f32; 2],
    ) -> ([f32; 2], [f32; 2]) {
        let half_w = VIRTUAL_WIDTH * 0.5;
        let half_h = VIRTUAL_HEIGHT * 0.5;
        let z = self.effective_zoom();

        let view_x = pos[0] - self.scroll[0] * scroll_factor[0];
        let view_y = pos[1] - self.scroll[1] * scroll_factor[1];

        let screen_x = (view_x - half_w) * z + half_w + self.shake_offset[0];
        let screen_y = (view_y - half_h) * z + half_h + self.shake_offset[1];
        ([screen_x, screen_y], [size[0] * z, size[1] * z])
    }

    #[inline]
    fn sync_scroll_from_follow(&mut self) {
        self.scroll[0] = self.follow_point[0] - VIRTUAL_WIDTH * 0.5;
        self.scroll[1] = self.follow_point[1] - VIRTUAL_HEIGHT * 0.5;
    }

    #[inline]
    fn next_signed_unit(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        ((x as f32) / (u32::MAX as f32)) * 2.0 - 1.0
    }
}

/// Paired `camGame` + `camHUD` controller managing V-Slice beat bop rate and intensity.
#[derive(Debug, Clone)]
pub struct CameraRig {
    pub cam_game: Camera2D,
    pub cam_hud: Camera2D,
    pub bop_intensity: f32,
    pub hud_bop_intensity: f32,
    pub zoom_rate_beats: f32,
    pub zoom_offset_beats: f32,
    pub zoom_enabled: bool,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self::new(DEFAULT_STAGE_ZOOM)
    }
}

impl CameraRig {
    /// Creates a [`CameraRig`] initialized with `stage_zoom` and default V-Slice bop settings.
    #[must_use]
    pub fn new(stage_zoom: f32) -> Self {
        Self {
            cam_game: Camera2D::new_game(stage_zoom),
            cam_hud: Camera2D::new_hud(),
            bop_intensity: DEFAULT_BOP_INTENSITY,
            hud_bop_intensity: DEFAULT_HUD_BOP_INTENSITY,
            zoom_rate_beats: DEFAULT_ZOOM_RATE as f32,
            zoom_offset_beats: 0.0,
            zoom_enabled: true,
        }
    }

    /// Updates both `cam_game` and `cam_hud` with zero allocations.
    pub fn update(&mut self, elapsed_s: f32) {
        if self.zoom_rate_beats > 0.0 {
            self.cam_game.update(elapsed_s);
            self.cam_hud.update(elapsed_s);
        } else {
            // Even when bopping is disabled, keep pan/zoom tweens, shake, and flash alive.
            let prev_bop = self.cam_game.bop_multiplier;
            let prev_hud_bop = self.cam_hud.hud_bop_offset;
            self.cam_game.update(elapsed_s);
            self.cam_hud.update(elapsed_s);
            self.cam_game.bop_multiplier = prev_bop;
            self.cam_hud.hud_bop_offset = prev_hud_bop;
        }
    }

    /// Triggers beat-aligned camera bops on `step_hit`, matching `PlayState.stepHit`.
    pub fn on_step_hit(&mut self, current_step: i32) {
        if !self.zoom_enabled || self.zoom_rate_beats <= 0.0 {
            return;
        }
        let steps_per_beat = STEPS_PER_BEAT as f32;
        let period = self.zoom_rate_beats * steps_per_beat;
        if period <= 0.0 {
            return;
        }
        let step_pos = (current_step as f32) + self.zoom_offset_beats * steps_per_beat;
        if (step_pos % period).abs() < 1e-4 {
            self.cam_game.bop(self.bop_intensity);
            self.cam_hud.bop_hud(self.hud_bop_intensity);
        }
    }

    /// Applies a `SetCameraBop` configuration (`SetCameraBopSongEvent.hx`).
    pub fn set_bop(&mut self, rate_beats: f32, offset_beats: f32, intensity: f32) {
        self.bop_intensity = (DEFAULT_BOP_INTENSITY - 1.0) * intensity + 1.0;
        self.hud_bop_intensity = (DEFAULT_BOP_INTENSITY - 1.0) * intensity * 2.0;
        self.zoom_rate_beats = rate_beats;
        self.zoom_offset_beats = offset_beats;
    }

    /// Resets both cameras to their defaults.
    pub fn reset(&mut self) {
        self.cam_game.reset(true, true, true);
        self.cam_hud.reset(true, true, true);
        self.bop_intensity = DEFAULT_BOP_INTENSITY;
        self.hud_bop_intensity = DEFAULT_HUD_BOP_INTENSITY;
        self.zoom_rate_beats = DEFAULT_ZOOM_RATE as f32;
        self.zoom_offset_beats = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn easing_parse_case_insensitive_and_with_dir() {
        assert_eq!(FlxEase::parse("LINEAR"), Some(FlxEase::Linear));
        assert_eq!(FlxEase::parse("instant"), Some(FlxEase::Instant));
        assert_eq!(FlxEase::parse("quadInOut"), Some(FlxEase::QuadInOut));
        assert_eq!(FlxEase::parse("CUBICOUT"), Some(FlxEase::CubicOut));
        assert_eq!(FlxEase::parse("cubeOut"), Some(FlxEase::CubicOut));
        assert_eq!(FlxEase::parse("smoothstepin"), Some(FlxEase::SmoothStepIn));
        assert_eq!(
            FlxEase::parse_with_dir("sine", Some("Out")),
            Some(FlxEase::SineOut)
        );
        assert_eq!(FlxEase::parse_with_dir("back", None), Some(FlxEase::BackIn));
        assert_eq!(
            FlxEase::parse_with_dir("linear", Some("Out")),
            Some(FlxEase::Linear)
        );
    }

    #[test]
    fn easing_endpoints_and_midpoints() {
        let all_continuous = [
            FlxEase::Linear,
            FlxEase::QuadIn,
            FlxEase::QuadOut,
            FlxEase::QuadInOut,
            FlxEase::CubicIn,
            FlxEase::CubicOut,
            FlxEase::CubicInOut,
            FlxEase::QuartIn,
            FlxEase::QuartOut,
            FlxEase::QuartInOut,
            FlxEase::QuintIn,
            FlxEase::QuintOut,
            FlxEase::QuintInOut,
            FlxEase::SineIn,
            FlxEase::SineOut,
            FlxEase::SineInOut,
            FlxEase::ExpoIn,
            FlxEase::ExpoOut,
            FlxEase::ExpoInOut,
            FlxEase::CircIn,
            FlxEase::CircOut,
            FlxEase::CircInOut,
            FlxEase::ElasticIn,
            FlxEase::ElasticOut,
            FlxEase::ElasticInOut,
            FlxEase::BackIn,
            FlxEase::BackOut,
            FlxEase::BackInOut,
            FlxEase::BounceIn,
            FlxEase::BounceOut,
            FlxEase::BounceInOut,
            FlxEase::SmoothStepIn,
            FlxEase::SmoothStepOut,
            FlxEase::SmoothStepInOut,
            FlxEase::SmootherStepIn,
            FlxEase::SmootherStepOut,
            FlxEase::SmootherStepInOut,
        ];

        for ease in all_continuous {
            assert!(approx(ease.eval(0.0), 0.0, 1e-4), "{ease:?} at 0.0");
            assert!(approx(ease.eval(1.0), 1.0, 1e-4), "{ease:?} at 1.0");
        }

        // Known midpoints at t = 0.5.
        assert!(approx(FlxEase::Linear.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::QuadIn.eval(0.5), 0.25, 1e-5));
        assert!(approx(FlxEase::QuadOut.eval(0.5), 0.75, 1e-5));
        assert!(approx(FlxEase::QuadInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::CubicInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::QuartInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::QuintInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::SineInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::ExpoInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::CircInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::ElasticInOut.eval(0.5), 0.5, 1e-4));
        assert!(approx(FlxEase::BackInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::BounceInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::SmoothStepInOut.eval(0.5), 0.5, 1e-5));
        assert!(approx(FlxEase::Instant.eval(0.0), 1.0, 1e-5));
    }

    #[test]
    fn camera_bop_exponential_decay() {
        let mut rig = CameraRig::new(1.0);
        rig.on_step_hit(0);
        assert!(approx(rig.cam_game.bop_multiplier, DEFAULT_BOP_INTENSITY, 1e-5));
        assert!(approx(rig.cam_hud.hud_bop_offset, DEFAULT_HUD_BOP_INTENSITY, 1e-5));

        // 1 frame at 60 FPS -> multiplied by 0.95 towards 1.0 / 0.0.
        rig.update(1.0 / 60.0);
        let expected_game = 1.0 + (DEFAULT_BOP_INTENSITY - 1.0) * 0.95;
        let expected_hud = DEFAULT_HUD_BOP_INTENSITY * 0.95;
        assert!(approx(rig.cam_game.bop_multiplier, expected_game, 1e-5));
        assert!(approx(rig.cam_hud.hud_bop_offset, expected_hud, 1e-5));

        // After ~2 seconds it decays close to resting state.
        for _ in 0..120 {
            rig.update(1.0 / 60.0);
        }
        assert!(approx(rig.cam_game.bop_multiplier, 1.0, 1e-3));
        assert!(approx(rig.cam_hud.hud_bop_offset, 0.0, 1e-3));
    }

    #[test]
    fn focus_camera_pan_tween_completes_cleanly() {
        let mut cam = Camera2D::new_game(1.0);
        cam.snap_to(640.0, 360.0);
        cam.tween_to(840.0, 260.0, 1.0, FlxEase::QuadOut);
        assert!(cam.is_pan_tweening());

        cam.update(0.5);
        // quadOut(0.5) = 0.75 => x = 640 + 200 * 0.75 = 790, y = 360 - 100 * 0.75 = 285.
        assert!(approx(cam.follow_point[0], 790.0, 1e-3));
        assert!(approx(cam.follow_point[1], 285.0, 1e-3));

        cam.update(0.5);
        assert!(!cam.is_pan_tweening());
        assert!(approx(cam.follow_point[0], 840.0, 1e-4));
        assert!(approx(cam.follow_point[1], 260.0, 1e-4));
        assert!(approx(cam.scroll[0], 200.0, 1e-4));
        assert!(approx(cam.scroll[1], -100.0, 1e-4));
    }

    #[test]
    fn world_to_screen_scroll_factors_and_zoom() {
        let mut cam = Camera2D::new_game(2.0);
        cam.snap_to(840.0, 460.0); // scroll = [200.0, 100.0]

        // scroll_factor [0, 0]: unaffected by scroll, zoomed from screen center (640, 360).
        let (p0, s0) = cam.world_to_screen([640.0, 360.0], [100.0, 50.0], [0.0, 0.0]);
        assert!(approx(p0[0], 640.0, 1e-4) && approx(p0[1], 360.0, 1e-4));
        assert!(approx(s0[0], 200.0, 1e-4) && approx(s0[1], 100.0, 1e-4));

        // scroll_factor [0.9, 0.9]: view = [500 - 180, 400 - 90] = [320, 310];
        // screen = [(320 - 640)*2 + 640, (310 - 360)*2 + 360] = [0, 260].
        let (p09, s09) = cam.world_to_screen([500.0, 400.0], [20.0, 20.0], [0.9, 0.9]);
        assert!(approx(p09[0], 0.0, 1e-4) && approx(p09[1], 260.0, 1e-4));
        assert!(approx(s09[0], 40.0, 1e-4) && approx(s09[1], 40.0, 1e-4));

        // scroll_factor [1, 1]: point at follow_point (840, 460) maps to screen center (640, 360).
        let (p1, _) = cam.world_to_screen([840.0, 460.0], [10.0, 10.0], [1.0, 1.0]);
        assert!(approx(p1[0], 640.0, 1e-4) && approx(p1[1], 360.0, 1e-4));
    }

    #[test]
    fn shake_and_flash_decay_to_zero() {
        let mut cam = Camera2D::new_game(1.0);
        cam.shake(0.05, 0.2);
        cam.flash([1.0, 0.5, 0.25, 0.8], 0.4);

        cam.update(0.1);
        assert!(cam.shake_duration_s > 0.0);
        assert!(approx(cam.flash_alpha, 0.6, 1e-4));

        cam.update(0.5);
        assert!(approx(cam.shake_duration_s, 0.0, 1e-5));
        assert!(approx(cam.shake_offset[0], 0.0, 1e-5));
        assert!(approx(cam.shake_offset[1], 0.0, 1e-5));
        assert!(cam.flash_overlay().is_none());
    }
}
