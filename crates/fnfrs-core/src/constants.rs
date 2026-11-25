//! V-Slice parity constants matching `funkin/util/Constants.hx`,
//! `funkin/play/notes/Strumline.hx`, and `funkin/play/notes/SustainTrail.hx`.

/// Virtual logical canvas width in pixels.
pub const VIRTUAL_WIDTH: f32 = 1280.0;

/// Virtual logical canvas height in pixels.
pub const VIRTUAL_HEIGHT: f32 = 720.0;

/// Scale multiplier for pixel-art sprites and stages.
pub const PIXEL_ART_SCALE: f32 = 6.0;

/// Base scroll factor in pixels per millisecond at `scroll_speed = 1.0`.
pub const PIXELS_PER_MS: f64 = 0.45;

/// Maximum timing window within which a note can be hit, in milliseconds.
pub const HIT_WINDOW_MS: f64 = 160.0;

/// Number of steps per beat (quarter note in 4/4).
pub const STEPS_PER_BEAT: u32 = 4;

/// Seconds per minute.
pub const SECS_PER_MIN: f64 = 60.0;

/// Milliseconds per second.
pub const MS_PER_SEC: f64 = 1000.0;

/// Standard MP3 decoder lead-in delay compensation at 44.1 kHz (`528 / 44100 * 1000`).
pub const MP3_DELAY_MS: f64 = (528.0 / 44100.0) * MS_PER_SEC;

/// Nominal receptor size in pixels (`Strumline.STRUMLINE_SIZE`).
pub const STRUMLINE_SIZE: f32 = 104.0;

/// Horizontal spacing between adjacent receptors (`STRUMLINE_SIZE + 8`).
pub const NOTE_SPACING: f32 = STRUMLINE_SIZE + 8.0;

/// Positional offset applied to receptor layout (`-0.275 * STRUMLINE_SIZE`).
pub const INITIAL_OFFSET: f32 = -0.275 * STRUMLINE_SIZE;

/// Minor horizontal alignment nudge for note sprites (`Strumline.NUDGE`).
pub const NUDGE: f32 = 2.0;

/// Standard 4-key lane count per strumline.
pub const KEY_COUNT: usize = 4;

/// Maximum concurrent note splashes per strumline.
pub const NOTE_SPLASH_CAP: usize = 6;

/// Horizontal margin of strumlines from screen edges/center.
pub const STRUMLINE_X_OFFSET: f32 = 48.0;

/// Vertical margin of strumlines from top/bottom screen edge.
pub const STRUMLINE_Y_OFFSET: f32 = 24.0;

/// Default sustain endcap height fraction (`SustainTrail.endOffset`).
pub const SUSTAIN_END_OFFSET: f32 = 0.5;

/// Default sustain bottom UV clip fraction (`SustainTrail.bottomClip`).
pub const SUSTAIN_BOTTOM_CLIP: f32 = 0.9;

/// Pixel-art sustain endcap height fraction.
pub const SUSTAIN_PIXEL_END_OFFSET: f32 = 1.0;

/// Pixel-art sustain bottom UV clip fraction.
pub const SUSTAIN_PIXEL_BOTTOM_CLIP: f32 = 1.0;

/// Default BPM when no time changes are specified.
pub const DEFAULT_BPM: f64 = 100.0;

/// Default time signature numerator.
pub const DEFAULT_TIME_SIGNATURE_NUM: u32 = 4;

/// Default time signature denominator.
pub const DEFAULT_TIME_SIGNATURE_DEN: u32 = 4;

/// Default chart difficulty.
pub const DEFAULT_DIFFICULTY: &str = "normal";

/// Default song variation.
pub const DEFAULT_VARIATION: &str = "default";

/// Default note style ID.
pub const DEFAULT_NOTE_STYLE: &str = "funkin";

/// Default stage ID.
pub const DEFAULT_STAGE: &str = "mainStage";

/// Default character ID.
pub const DEFAULT_CHARACTER: &str = "bf";

/// Default health icon ID.
pub const DEFAULT_HEALTH_ICON: &str = "face";

/// Maximum player health (`Constants.HEALTH_MAX`).
pub const HEALTH_MAX: f32 = 2.0;

/// Starting player health (`HEALTH_MAX / 2.0`).
pub const HEALTH_STARTING: f32 = HEALTH_MAX / 2.0;

/// Minimum player health.
pub const HEALTH_MIN: f32 = 0.0;

/// Health gained for a Killer judgement (`+2.0%` of `HEALTH_MAX`).
pub const HEALTH_KILLER_BONUS: f32 = 0.02 * HEALTH_MAX;

/// Health gained for a Sick judgement (`+1.5%` of `HEALTH_MAX`).
pub const HEALTH_SICK_BONUS: f32 = 0.015 * HEALTH_MAX;

/// Health gained for a Good judgement (`+0.75%` of `HEALTH_MAX`).
pub const HEALTH_GOOD_BONUS: f32 = 0.0075 * HEALTH_MAX;

/// Health gained for a Bad judgement.
pub const HEALTH_BAD_BONUS: f32 = 0.0;

/// Health change for a Shit judgement (`-1.0%` of `HEALTH_MAX`).
pub const HEALTH_SHIT_BONUS: f32 = -0.01 * HEALTH_MAX;

/// Health gained per second while holding a sustain note (`+6.0%` / s of `HEALTH_MAX`).
pub const HEALTH_HOLD_BONUS_PER_SECOND: f32 = 0.06 * HEALTH_MAX;

/// Health lost on a missed note (`-4.0%` of `HEALTH_MAX`).
pub const HEALTH_MISS_PENALTY: f32 = -0.04 * HEALTH_MAX;

/// Health lost on a ghost miss (`-4.0%` of `HEALTH_MAX`).
pub const HEALTH_GHOST_MISS_PENALTY: f32 = -0.04 * HEALTH_MAX;

/// Health lost per second when dropping a sustain note early (`Constants.HEALTH_HOLD_DROP_PENALTY_PER_SECOND`).
pub const HEALTH_HOLD_DROP_PENALTY_PER_SECOND: f32 = 0.0 * HEALTH_MAX;

/// Maximum health lost for dropping a sustain note early (`Constants.HEALTH_HOLD_DROP_PENALTY_MAX`).
pub const HEALTH_HOLD_DROP_PENALTY_MAX: f32 = 0.0 * HEALTH_MAX;

/// Grace period in seconds after hitting a note before ghost-tapping penalizes (`Constants.GHOST_TAP_DELAY`).
pub const GHOST_TAP_DELAY: f64 = 3.0 / 8.0;

/// Score penalty on ghost miss.
pub const SCORE_GHOST_MISS_PENALTY: f32 = -10.0;

/// Score gained per second while holding a sustain note.
pub const SCORE_HOLD_BONUS_PER_SECOND: f32 = 250.0;

/// Score lost per remaining second when dropping a sustain note early.
pub const SCORE_HOLD_DROP_PENALTY_PER_SECOND: f32 = -125.0;

/// Minimum remaining sustain length in ms before dropping incurs a penalty.
pub const HOLD_DROP_PENALTY_THRESHOLD_MS: f64 = 160.0;

/// Default camera bop zoom multiplier.
pub const DEFAULT_BOP_INTENSITY: f32 = 1.015;

/// Default camera zoom bop rate in beats.
pub const DEFAULT_ZOOM_RATE: u32 = 4;

/// Default camera follow lerp rate per 60Hz tick.
pub const DEFAULT_CAMERA_FOLLOW_RATE: f32 = 0.04;

/// Default countdown audio volume.
pub const COUNTDOWN_VOLUME: f32 = 0.6;

/// Non-pixel health icon nominal size.
pub const HEALTH_ICON_SIZE: f32 = 150.0;

/// Pixel health icon nominal size.
pub const PIXEL_ICON_SIZE: f32 = 32.0;

/// Health icon bop scale boost.
pub const HEALTH_ICON_BOP_SCALE: f32 = 0.2;

/// Health icon angle decay half-life.
pub const HEALTH_ICON_ANGLE_HALF_LIFE: f32 = 0.512;

/// Completion ratio required for `PERFECT` rank.
pub const RANK_PERFECT_THRESHOLD: f64 = 1.00;

/// Completion ratio required for `EXCELLENT` rank.
pub const RANK_EXCELLENT_THRESHOLD: f64 = 0.90;

/// Completion ratio required for `GREAT` rank.
pub const RANK_GREAT_THRESHOLD: f64 = 0.80;

/// Completion ratio required for `GOOD` rank.
pub const RANK_GOOD_THRESHOLD: f64 = 0.60;
