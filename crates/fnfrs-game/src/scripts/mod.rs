//! Native Rust implementations of base Friday Night Funkin' V-Slice `.hxc` scripts
//! (characters, note kinds, and stage dynamics) with zero per-frame heap allocations.

mod abot;
mod abot_vis;
mod blazin;
mod speaker;
mod spirit;
mod spraycan;
mod stages_events;
mod stages_runtime;
mod stages_setup;
mod tankmen;

use std::sync::Arc;

use fnfrs_audio::{DecodedAudio, VizSampleHistory};
use fnfrs_render::TextureHandle;

use abot::ABotSpeaker;
pub use blazin::BlazinNoteKind;
use blazin::{parse_blazin_note_kind, BlazinFighterState};
use speaker::SpeakerShooter;
use spirit::SpiritTrail;
use spraycan::{QuadCarTween, SpraycanPool};
use crate::character::BaseCharacter;
use tankmen::TankmanRunners;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteKindTag {
    #[default]
    Normal,
    Alt,
    Mom,
    Cheer,
    NoAnim,
    Ugh,
    HehPrettyGood,
    Weekend1LightCan,
    Weekend1KickCan,
    Weekend1KneeCan,
    Weekend1CockGun,
    Weekend1FireGun,
    Blazin(BlazinNoteKind),
    Other,
}

impl NoteKindTag {
    /// Parses a chart note's optional `kind` string at song load time.
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(k) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Normal;
        };
        if k.eq_ignore_ascii_case("mom") {
            Self::Mom
        } else if k.eq_ignore_ascii_case("alt") || k.eq_ignore_ascii_case("-alt") {
            Self::Alt
        } else if k.eq_ignore_ascii_case("cheer") {
            Self::Cheer
        } else if k.eq_ignore_ascii_case("noanim") {
            Self::NoAnim
        } else if k.eq_ignore_ascii_case("ugh") {
            Self::Ugh
        } else if k.eq_ignore_ascii_case("hehPrettyGood") {
            Self::HehPrettyGood
        } else if k.eq_ignore_ascii_case("weekend-1-lightcan") {
            Self::Weekend1LightCan
        } else if k.eq_ignore_ascii_case("weekend-1-kickcan") {
            Self::Weekend1KickCan
        } else if k.eq_ignore_ascii_case("weekend-1-kneecan") {
            Self::Weekend1KneeCan
        } else if k.eq_ignore_ascii_case("weekend-1-cockgun") {
            Self::Weekend1CockGun
        } else if k.eq_ignore_ascii_case("weekend-1-firegun")
            || k.eq_ignore_ascii_case("weekend-1-firegun-hip")
            || k.eq_ignore_ascii_case("weekend-1-firegun-far")
        {
            Self::Weekend1FireGun
        } else if let Some(blazin_kind) = parse_blazin_note_kind(k) {
            Self::Blazin(blazin_kind)
        } else {
            Self::Other
        }
    }
}

/// Optional SFX audio slot indices for stage & character `.hxc` scripts.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScriptSfxSlots {
    pub thunder: [Option<usize>; 2],
    pub train_passes: Option<usize>,
    pub car_pass: [Option<usize>; 2],
    pub lightning: [Option<usize>; 3],
    pub door_kick: [Option<usize>; 2],
    pub gun_prep: Option<usize>,
    pub pico_bonk: Option<usize>,
    pub shot: [Option<usize>; 4],
}

/// Runtime SFX cue emitted cleanly by [`NativeScriptHost::update`] / `on_beat_hit`.
#[derive(Debug, Clone, Copy)]
pub struct ScriptSfxCue {
    pub slot: usize,
    pub gain: f32,
}

/// Fixed-capacity cue buffer emitted by scripts per step (avoids Vec allocations).
#[derive(Debug, Clone, Copy, Default)]
pub struct ScriptStepOutput {
    pub cues: [Option<ScriptSfxCue>; 4],
    pub count: usize,
    pub camera_shake: Option<[f32; 2]>,
}

impl ScriptStepOutput {
    pub fn push(&mut self, slot: usize, gain: f32) {
        if self.count < self.cues.len() {
            self.cues[self.count] = Some(ScriptSfxCue { slot, gain });
            self.count += 1;
        }
    }
}

/// Deterministic fast xorshift32 PRNG matching zero-alloc gameplay style.
#[derive(Debug, Clone)]
pub(super) struct ScriptRng(u32);

impl ScriptRng {
    pub(super) fn next_u32(&mut self) -> u32 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    pub(super) fn bool_pct(&mut self, percent: u32) -> bool {
        (self.next_u32() % 100) < percent
    }

    pub(super) fn range_i32(&mut self, min: i32, max_inclusive: i32) -> i32 {
        if max_inclusive <= min {
            return min;
        }
        let span = (max_inclusive - min + 1) as u32;
        min + (self.next_u32() % span) as i32
    }

    pub(super) fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        let t = (self.next_u32() as f32) / (u32::MAX as f32);
        min + (max - min) * t
    }
}

#[derive(Debug)]
pub(super) enum StageScript {
    None,
    SpookyMansion {
        is_erect: bool,
        lightning_strike_beat: i32,
        lightning_strike_offset: i32,
        flash_timer_s: Option<f32>,
    },
    PhillyTrain {
        light_alpha: f32,
        cur_light: usize,
        train_moving: bool,
        train_started: bool,
        train_finishing: bool,
        train_cars: i32,
        train_cooldown: i32,
        train_timer_s: f32,
        train_sound_ms: f32,
    },
    PhillyBlazin {
        sky_blur_tex: Option<(TextureHandle, [f32; 2])>,
        sky_scroll_x: f32,
        rain_time_scale: f32,
        lightning_timer_s: f32,
        lightning_fade_s: f32,
        lightning_fading: bool,
    },
    LimoRide {
        fast_car_can_drive: bool,
        fast_car_active: bool,
        fast_car_timer_s: f32,
        fast_car_vx: f32,
    },
    School,
    SchoolEvil {
        time_s: f32,
    },
    TankmanBattlefield {
        is_erect: bool,
        clouds_tex: Option<(TextureHandle, [f32; 2])>,
        clouds_x: f32,
        tank_angle: f32,
        tank_speed: f32,
    },
    PhillyStreets {
        sky_tex: Option<(TextureHandle, [f32; 2])>,
        sky_scroll_x: f32,
        lights_stop: bool,
        last_change: i32,
        change_interval: i32,
        car_waiting: bool,
        car_interruptable: bool,
        car2_interruptable: bool,
        car1: QuadCarTween,
        car2: QuadCarTween,
    },
    Sserafim {
        yunjin: Option<Box<BaseCharacter>>,
        chaewon: Option<Box<BaseCharacter>>,
        eunchae: Option<Box<BaseCharacter>>,
        dust_mid_tex: Option<(TextureHandle, [f32; 2])>,
        dust_back_tex: Option<(TextureHandle, [f32; 2])>,
        dust_time_s: f32,
        visible: [bool; 6],
        singing: [bool; 6],
        lights_enabled: bool,
        light_colors: [[f32; 4]; 8],
        light_durs: [f32; 8],
        light_ints: [f32; 8],
        light_count: usize,
        light_idx: usize,
        pulse_alpha: f32,
        pulse_dur: f32,
        truck_light_alpha: f32,
        truck_light_dur: f32,
        cover_alpha: f32,
        dark_amount: f32,
        dark_start: f32,
        dark_target: f32,
        dark_elapsed: f32,
        dark_dur: f32,
        kick_waiting_door: bool,
    },
}

/// Active fullscreen `RuntimeRainShader` parameters passed to `RainPostPass`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainShaderParams {
    pub time: f32,
    pub scale: f32,
    pub intensity: f32,
    pub color: [f32; 3],
}

/// Coordinates all hardcoded V-Slice stage and character `.hxc` behaviors cleanly
/// with zero heap allocations inside update/render loops.
#[derive(Debug)]
pub struct NativeScriptHost {
    song_id: String,
    stage_id_lower: String,
    dad_is_gf: bool,
    stage_script: StageScript,
    spirit_trail: SpiritTrail,
    speaker_shooter: SpeakerShooter,
    abot: ABotSpeaker,
    runners: TankmanRunners,
    spraycans: SpraycanPool,
    blazin: BlazinFighterState,
    sfx: ScriptSfxSlots,
    rng: ScriptRng,
    rain_enabled: bool,
    rain_time: f32,
    rain_scale: f32,
    rain_intensity: f32,
    rain_start_intensity: f32,
    rain_end_intensity: f32,
    rain_duration_ms: f64,
    rain_color: [f32; 3],
}


impl NativeScriptHost {

    /// Minimal empty host for tests & headless scenes without a loaded stage script.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            song_id: String::new(),
            stage_id_lower: String::new(),
            dad_is_gf: false,
            stage_script: StageScript::None,
            spirit_trail: SpiritTrail::default(),
            speaker_shooter: SpeakerShooter::default(),
            abot: ABotSpeaker::default(),
            runners: TankmanRunners::default(),
            spraycans: SpraycanPool::default(),
            blazin: BlazinFighterState::default(),
            sfx: ScriptSfxSlots::default(),
            rng: ScriptRng(0x5A17_C0DE),
            rain_enabled: false,
            rain_time: 0.0,
            rain_scale: 3.6,
            rain_intensity: 0.0,
            rain_start_intensity: 0.0,
            rain_end_intensity: 0.0,
            rain_duration_ms: 180_000.0,
            rain_color: [0.659, 0.678, 0.710],
        }
    }

    /// Sets the instrumental audio buffer used by A-Bot's FFT visualizer (`ABotVis.hx`).
    pub fn set_visualizer_audio(&mut self, inst_audio: Option<(DecodedAudio, i64)>) {
        self.abot.analyzer.inst_audio = inst_audio;
        self.abot.analyzer.reset();
    }

    /// Connects the lock-free streaming PCM ring buffer for A-Bot's FFT visualizer.
    pub fn set_visualizer_history(&mut self, history: Option<Arc<VizSampleHistory>>, sample_rate_hz: u32) {
        self.abot.analyzer.viz_history = history;
        self.abot.analyzer.viz_sample_rate_hz = sample_rate_hz;
        self.abot.analyzer.reset();
    }

    /// Sets whether the visualizer is currently audible (`FlxG.sound.volume > 0 && !muted`).
    #[inline]
    pub fn set_visualizer_audible(&mut self, audible: bool) {
        self.abot.analyzer.audible = audible;
    }

    /// Sets song duration for gradual Weekend 1 rain intensity interpolation.
    pub fn set_song_duration_ms(&mut self, duration_ms: f64) {
        if duration_ms > 1.0 {
            self.rain_duration_ms = duration_ms;
        }
    }

    /// Returns active fullscreen rain shader uniforms when rendering `phillyStreetsErect` / `phillyBlazin` / `spookyMansionErect`.
    #[must_use]
    pub fn rain_params(&self) -> Option<RainShaderParams> {
        if self.rain_enabled && self.rain_intensity > 0.0 {
            Some(RainShaderParams {
                time: self.rain_time,
                scale: self.rain_scale,
                intensity: self.rain_intensity,
                color: self.rain_color,
            })
        } else {
            None
        }
    }

    /// True for songs/stages that hide opponent strumline notes (`blazin`, `sserafim`/`spaghetti`).
    #[must_use]
    pub fn hides_opponent_strumline(&self) -> bool {
        self.song_id.eq_ignore_ascii_case("blazin")
            || self.song_id.eq_ignore_ascii_case("spaghetti")
            || self.stage_id_lower == "sserafim"
    }

    /// True for songs that center player strumline notes horizontally (`blazin`).
    #[must_use]
    pub fn centers_player_strumline(&self) -> bool {
        self.song_id.eq_ignore_ascii_case("blazin")
    }

    /// Returns `true` when the active stage/character script manages GF's beat dancing directly.
    #[must_use]
    pub fn owns_gf_dance(&self) -> bool {
        self.abot.nene_knife_enabled
    }

    /// True for stages that suppress standard combo popups (`sserafim`).
    #[must_use]
    pub fn hides_popups(&self) -> bool {
        self.stage_id_lower == "sserafim"
    }

}
