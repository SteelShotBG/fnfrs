//! Native Rust implementations of base Friday Night Funkin' V-Slice `.hxc` scripts
//! (characters, note kinds, and stage dynamics) with zero per-frame heap allocations.

use std::f32::consts::PI;

use fnfrs_audio::DecodedAudio;
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BlendMode, EvaluatedPart, FilterMode, GpuContext, SparrowAtlas,
    SpriteBatcher, SpriteInstance, TextureHandle, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_ROTATED_90_CCW,
};

use crate::character::{
    apply_camera_transform, compute_flipped_trim_offset, draw_animate_parts, load_animate_source,
    upload_png_path, AtlasDrawParams, BaseCharacter, CharacterKind, LoadedAtlasSource,
};
use crate::input::NoteDirection;
use crate::stage::{parse_stage_color, Stage};

/// Compact `Copy` tag classifying V-Slice `note.kind` strings without hot-loop allocations.
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
        } else if k.eq_ignore_ascii_case("weekend-1-firegun") {
            Self::Weekend1FireGun
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
struct ScriptRng(u32);

impl ScriptRng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn bool_pct(&mut self, percent: u32) -> bool {
        (self.next_u32() % 100) < percent
    }

    fn range_i32(&mut self, min: i32, max_inclusive: i32) -> i32 {
        if max_inclusive <= min {
            return min;
        }
        let span = (max_inclusive - min + 1) as u32;
        min + (self.next_u32() % span) as i32
    }

    fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        let t = (self.next_u32() as f32) / (u32::MAX as f32);
        min + (max - min) * t
    }
}

// ============================================================================
// 1. Spirit Ghost Trail (`spirit.hxc` + `FunkTrail.hx`)
// ============================================================================

#[derive(Debug, Clone, Copy)]
struct SpiritGhostSample {
    valid: bool,
    anim_index: usize,
    frame_index: usize,
    position: [f32; 2],
    anim_offsets: [f32; 2],
    flip_x: bool,
}

impl Default for SpiritGhostSample {
    fn default() -> Self {
        Self {
            valid: false,
            anim_index: 0,
            frame_index: 0,
            position: [0.0, 0.0],
            anim_offsets: [0.0, 0.0],
            flip_x: false,
        }
    }
}

#[derive(Debug)]
struct SpiritTrail {
    enabled: bool,
    z_index: i32,
    delay_s: f32,
    base_alpha: f32,
    alpha_diff: f32,
    timer_s: f32,
    samples: [SpiritGhostSample; 4],
}

impl Default for SpiritTrail {
    fn default() -> Self {
        Self {
            enabled: false,
            z_index: 190,
            delay_s: 0.4,
            base_alpha: 0.3,
            alpha_diff: 0.069,
            timer_s: 0.0,
            samples: [SpiritGhostSample::default(); 4],
        }
    }
}

impl SpiritTrail {
    fn reset(&mut self) {
        self.timer_s = 0.0;
        self.samples = [SpiritGhostSample::default(); 4];
    }

    fn update(&mut self, elapsed_s: f32, dad: Option<&BaseCharacter>) {
        if !self.enabled {
            return;
        }
        let Some(dad) = dad else {
            return;
        };
        self.timer_s += elapsed_s.max(0.0);
        if self.timer_s < self.delay_s {
            return;
        }
        self.timer_s -= self.delay_s;

        // Shift trail buffer backward without allocating.
        for i in (1..self.samples.len()).rev() {
            self.samples[i] = self.samples[i - 1];
        }
        if let Some((anim_idx, frame_idx, offsets)) = dad.active_frame_snapshot() {
            self.samples[0] = SpiritGhostSample {
                valid: true,
                anim_index: anim_idx,
                frame_index: frame_idx,
                position: dad.position,
                anim_offsets: offsets,
                flip_x: dad.flip_x,
            };
        }
    }

    fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32, dad: Option<&BaseCharacter>) {
        if !self.enabled {
            return;
        }
        let Some(dad) = dad else {
            return;
        };
        for (i, sample) in self.samples.iter().enumerate().rev() {
            if !sample.valid {
                continue;
            }
            let alpha = (self.base_alpha - self.alpha_diff * (i as f32)).max(0.0);
            if alpha <= 0.001 {
                continue;
            }
            dad.draw_snapshot_ghost(
                batcher,
                cam_pos,
                cam_zoom,
                sample.anim_index,
                sample.frame_index,
                sample.position,
                sample.anim_offsets,
                sample.flip_x,
                alpha,
            );
        }
    }
}

// ============================================================================
// 2. Pico Speaker / Otis Speaker (`pico-speaker.hxc` & `otis-speaker.hxc`)
// ============================================================================

#[derive(Debug, Default)]
struct SpeakerShooter {
    enabled: bool,
    notes: Vec<(f64, u8)>,
    cursor: usize,
}

impl SpeakerShooter {
    fn new(gf_id: &str, picospeaker_notes: &[SongNoteData]) -> Self {
        let enabled = gf_id.eq_ignore_ascii_case("pico-speaker")
            || gf_id.eq_ignore_ascii_case("otis-speaker");
        if !enabled {
            return Self::default();
        }
        let mut notes: Vec<(f64, u8)> = picospeaker_notes
            .iter()
            .map(|n| (n.time, n.data as u8))
            .collect();
        notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self {
            enabled,
            notes,
            cursor: 0,
        }
    }

    fn reset(&mut self) {
        self.cursor = 0;
    }

    fn update(&mut self, song_pos_ms: f64, gf: Option<&mut BaseCharacter>, rng: &mut ScriptRng) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let gf = gf?;
        let mut fired_dir = None;
        while self.cursor < self.notes.len() && self.notes[self.cursor].0 <= song_pos_ms {
            let raw_dir = self.notes[self.cursor].1;
            self.cursor += 1;
            let delta = rng.range_i32(0, 1);
            let dir = if raw_dir == 3 {
                (3 - delta).clamp(0, 3)
            } else {
                ((raw_dir as i32) + delta).clamp(0, 3)
            };
            let anim = match dir {
                0 => "shoot1",
                1 => "shoot2",
                2 => "shoot3",
                _ => "shoot4",
            };
            gf.play_animation(anim, true, true);
            fired_dir = Some(dir as usize);
        }
        fired_dir
    }
}

// ============================================================================
// 3. A-Bot Speaker & Nene State Machine (`nene.hxc`, `otis-speaker.hxc`, `ABotVis.hx`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PupilLook {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NeneState {
    #[default]
    Default,
    PreRaise,
    Raise,
    Ready,
    Lower,
    HairBlowing,
    HairFalling,
    HairBlowingRaise,
    HairFallingRaise,
}

#[derive(Debug)]
struct LoadedAnimateProp {
    atlas: AnimateAtlas,
    textures: Vec<(TextureHandle, [f32; 2])>,
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    frame_time_s: f32,
    current_frame: u32,
    playing: bool,
    parts_buf: Vec<EvaluatedPart>,
}

impl LoadedAnimateProp {
    fn load(assets: &AssetLocator, gpu: &mut GpuContext, rel_dir: &str) -> Option<Self> {
        let dir = assets.animate_atlas_dir(rel_dir);
        let anim_json = std::fs::read_to_string(dir.join("Animation.json")).ok()?;
        let mut sm_entries: Vec<(u32, std::path::PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("spritemap") && name.ends_with(".json") {
                    let num = name["spritemap".len()..name.len() - ".json".len()]
                        .parse::<u32>()
                        .unwrap_or(0);
                    sm_entries.push((num, p));
                }
            }
        }
        sm_entries.sort_by_key(|(n, _)| *n);
        if sm_entries.is_empty() {
            return None;
        }
        let mut sm_strings = Vec::with_capacity(sm_entries.len());
        for (_, p) in &sm_entries {
            sm_strings.push(std::fs::read_to_string(p).ok()?);
        }
        let sm_refs: Vec<&str> = sm_strings.iter().map(String::as_str).collect();
        let atlas = AnimateAtlas::from_json_strs(&anim_json, &sm_refs).ok()?;
        let mut textures = Vec::with_capacity(atlas.spritemaps().len());
        for sm in atlas.spritemaps() {
            textures.push(upload_png_path(gpu, Some(&sm.image), &dir.join(&sm.image))?);
        }
        let (bounds_origin, bounds_size) = atlas.full_timeline_bounds();
        let mut prop = Self {
            atlas,
            textures,
            bounds_origin,
            bounds_size,
            frame_time_s: 0.0,
            current_frame: 0,
            playing: true,
            parts_buf: Vec::with_capacity(64),
        };
        prop.atlas.evaluate_frame(0, &mut prop.parts_buf);
        Some(prop)
    }

    fn play_from(&mut self, start_frame: u32) {
        let total = self.atlas.total_frames().max(1);
        self.current_frame = start_frame.min(total - 1);
        self.frame_time_s = self.current_frame as f32 / self.atlas.frame_rate().max(1.0);
        self.playing = true;
        self.atlas.evaluate_frame(self.current_frame, &mut self.parts_buf);
    }

    fn update_stopped_at(&mut self, elapsed_s: f32, pause_frame: Option<u32>) {
        if !self.playing {
            return;
        }
        let total = self.atlas.total_frames().max(1);
        let fps = self.atlas.frame_rate().max(1.0);
        self.frame_time_s += elapsed_s.max(0.0);
        let target = (self.frame_time_s * fps).floor() as u32;
        if let Some(pf) = pause_frame {
            if self.current_frame < pf && target >= pf {
                self.current_frame = pf;
                self.playing = false;
                self.atlas.evaluate_frame(pf, &mut self.parts_buf);
                return;
            }
        }
        let clamped = target.min(total - 1);
        if clamped != self.current_frame {
            self.current_frame = clamped;
            self.atlas.evaluate_frame(clamped, &mut self.parts_buf);
        }
        if target >= total - 1 {
            self.playing = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        pos: [f32; 2],
        scale: f32,
        scroll: [f32; 2],
        cam_pos: [f32; 2],
        cam_zoom: f32,
        color: [f32; 4],
        adjust_color: Option<[f32; 4]>,
    ) {
        let params = AtlasDrawParams {
            pos,
            scale: [scale, scale],
            anim_offsets: [0.0, 0.0],
            global_offsets: [0.0, 0.0],
            scroll_factor: scroll,
            cam_pos,
            cam_zoom,
            angle_deg: 0.0,
            flip_x: false,
            flip_y: false,
            color,
            alpha: color[3],
            blend: BlendMode::Normal,
            is_pixel: false,
            adjust_color,
            wiggle: None,
            ref_unscaled_size: None,
        };
        draw_animate_parts(
            batcher,
            &self.textures,
            &self.parts_buf,
            self.bounds_origin,
            self.bounds_size,
            None,
            &params,
            false,
            false,
        );
    }
}

const VIZ_FFT_N: usize = 256;
const VIZ_HALF_N: usize = VIZ_FFT_N / 2;
const VIZ_BAR_COUNT: usize = 7;
const VIZ_MIN_DB: f32 = -65.0;
const VIZ_DB_RANGE: f32 = 40.0;
const VIZ_MIN_FREQ: f32 = 10.0;
const VIZ_MAX_FREQ: f32 = 22000.0;
const VIZ_MAX_DELTA: f32 = 0.1;
const VIZ_DB_PER_LN: f32 = 8.685_889;
const VIZ_FFT_NORM: f32 = (2.0 / VIZ_FFT_N as f32) * (8.0 / 12.0);

/// Zero-allocation radix-2 FFT & 7-band logarithmic spectrum analyzer
/// matching `FunkinCrew/funkVis` (`SpectralAnalyzer` + `ABotVis.hx`).
#[derive(Debug)]
struct ABotSpectralAnalyzer {
    inst_audio: Option<(DecodedAudio, i64)>,
    audible: bool,
    window: [f32; VIZ_FFT_N],
    bit_rev: [u8; VIZ_FFT_N],
    re: [f32; VIZ_FFT_N],
    im: [f32; VIZ_FFT_N],
    mag: [f32; VIZ_HALF_N],
    bar_values: [f32; VIZ_BAR_COUNT],
}

impl Default for ABotSpectralAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl ABotSpectralAnalyzer {
    fn new() -> Self {
        let mut window = [0.0_f32; VIZ_FFT_N];
        let mut bit_rev = [0_u8; VIZ_FFT_N];
        for i in 0..VIZ_FFT_N {
            window[i] = 1.0 - 0.85 * (2.0 * PI * (i as f32) / (VIZ_FFT_N as f32)).cos();
            bit_rev[i] = (i as u8).reverse_bits();
        }
        Self {
            inst_audio: None,
            audible: true,
            window,
            bit_rev,
            re: [0.0; VIZ_FFT_N],
            im: [0.0; VIZ_FFT_N],
            mag: [0.0; VIZ_HALF_N],
            bar_values: [0.0; VIZ_BAR_COUNT],
        }
    }

    fn reset(&mut self) {
        self.bar_values = [0.0; VIZ_BAR_COUNT];
    }

    fn analyze(
        &mut self,
        song_pos_ms: f64,
        elapsed_s: f32,
        out_levels: &mut [u8; VIZ_BAR_COUNT],
    ) {
        if !self.audible || song_pos_ms < 0.0 {
            self.bar_values = [0.0; VIZ_BAR_COUNT];
            *out_levels = [0; VIZ_BAR_COUNT];
            return;
        }
        let Some((audio, offset_frames)) = &self.inst_audio else {
            self.bar_values = [0.0; VIZ_BAR_COUNT];
            *out_levels = [0; VIZ_BAR_COUNT];
            return;
        };
        if audio.frames == 0 || audio.sample_rate == 0 {
            *out_levels = [0; VIZ_BAR_COUNT];
            return;
        }

        let sr_f64 = f64::from(audio.sample_rate);
        let end_frame = ((song_pos_ms / 1000.0) * sr_f64).round() as i64 - *offset_frames;
        if end_frame <= 0 {
            self.bar_values = [0.0; VIZ_BAR_COUNT];
            *out_levels = [0; VIZ_BAR_COUNT];
            return;
        }
        let start_frame = end_frame - (VIZ_FFT_N as i64);

        for i in 0..VIZ_FFT_N {
            let f = start_frame + (i as i64);
            let mono = if f >= 0 && (f as usize) < audio.frames {
                let s_idx = (f as usize) * 2;
                (audio.samples[s_idx] + audio.samples[s_idx + 1]) * 0.5
            } else {
                0.0
            };
            let dst = self.bit_rev[i] as usize;
            self.re[dst] = mono * self.window[i];
            self.im[dst] = 0.0;
        }

        // In-place Cooley-Tukey radix-2 DIT FFT (zero heap allocations).
        let mut size = 2usize;
        while size <= VIZ_FFT_N {
            let half = size / 2;
            let angle_step = -2.0 * PI / (size as f32);
            let mut start = 0usize;
            while start < VIZ_FFT_N {
                for j in 0..half {
                    let (sin_a, cos_a) = (angle_step * (j as f32)).sin_cos();
                    let even = start + j;
                    let odd = even + half;
                    let t_re = self.re[odd] * cos_a - self.im[odd] * sin_a;
                    let t_im = self.re[odd] * sin_a + self.im[odd] * cos_a;
                    self.re[odd] = self.re[even] - t_re;
                    self.im[odd] = self.im[even] - t_im;
                    self.re[even] += t_re;
                    self.im[even] += t_im;
                }
                start += size;
            }
            size <<= 1;
        }

        for k in 0..VIZ_HALF_N {
            self.mag[k] = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt()
                * VIZ_FFT_NORM;
        }

        let sr_f32 = audio.sample_rate as f32;
        let freq_ratio = VIZ_MAX_FREQ / VIZ_MIN_FREQ;
        let dt_frames = (elapsed_s.max(0.0) * 60.0).clamp(0.0, 4.0);
        let max_fall = VIZ_MAX_DELTA * dt_frames.max(0.25);

        for b in 0..VIZ_BAR_COUNT {
            let f_lo = VIZ_MIN_FREQ * freq_ratio.powf((b as f32) / (VIZ_BAR_COUNT as f32));
            let f_hi =
                VIZ_MIN_FREQ * freq_ratio.powf(((b + 1) as f32) / (VIZ_BAR_COUNT as f32));
            let bin_lo = (f_lo * (VIZ_FFT_N as f32) / sr_f32).clamp(0.0, (VIZ_HALF_N - 1) as f32);
            let bin_hi =
                (f_hi * (VIZ_FFT_N as f32) / sr_f32).clamp(bin_lo, (VIZ_HALF_N - 1) as f32);

            let mid = ((bin_lo + bin_hi) * 0.5)
                .max(0.75)
                .clamp(0.0, (VIZ_HALF_N - 1) as f32);
            let m0 = mid.floor() as usize;
            let m1 = (m0 + 1).min(VIZ_HALF_N - 1);
            let frac = mid - (m0 as f32);
            let mut amp = self.mag[m0] * (1.0 - frac) + self.mag[m1] * frac;

            let i_lo = (bin_lo.ceil() as usize).max(1);
            let i_hi = (bin_hi.floor() as usize).min(VIZ_HALF_N - 1);
            if i_lo <= i_hi {
                for &m in &self.mag[i_lo..=i_hi] {
                    if m > amp {
                        amp = m;
                    }
                }
            }

            let db = if amp > 1e-12 {
                VIZ_DB_PER_LN * amp.ln()
            } else {
                VIZ_MIN_DB
            };
            let raw_norm = ((db - VIZ_MIN_DB) / VIZ_DB_RANGE).clamp(0.0, 1.0);
            let prev = self.bar_values[b];
            let next = if raw_norm >= prev {
                raw_norm
            } else {
                (prev - max_fall).max(raw_norm)
            };
            self.bar_values[b] = next;
            out_levels[b] = (next * 6.0).round().clamp(0.0, 6.0) as u8;
        }
    }
}

#[derive(Debug)]
struct ABotSpeaker {
    enabled: bool,
    is_dark: bool,
    is_otis: bool,
    is_tankmen: bool,
    gf_z: i32,
    abot: Option<LoadedAnimateProp>,
    eyes: Option<LoadedAnimateProp>,
    stereo_bg: Option<(TextureHandle, [f32; 2])>,
    white_tex: TextureHandle,
    viz_tex: Option<(TextureHandle, [f32; 2])>,
    viz_bars: [Vec<AtlasFrame>; 7],
    viz_levels: [u8; 7],
    analyzer: ABotSpectralAnalyzer,
    muzzle_tex: Option<(TextureHandle, [f32; 2])>,
    muzzle_anims: [Vec<AtlasFrame>; 4],
    muzzle_active: bool,
    muzzle_dir: usize,
    muzzle_offset: [f32; 2],
    muzzle_time_s: f32,
    pupil_state: PupilLook,
    nene_knife_enabled: bool,
    nene_state: NeneState,
    blink_countdown: i32,
}

impl Default for ABotSpeaker {
    fn default() -> Self {
        Self {
            enabled: false,
            is_dark: false,
            is_otis: false,
            is_tankmen: false,
            gf_z: 300,
            abot: None,
            eyes: None,
            stereo_bg: None,
            white_tex: TextureHandle(0),
            viz_tex: None,
            viz_bars: std::array::from_fn(|_| Vec::new()),
            viz_levels: [0; 7],
            analyzer: ABotSpectralAnalyzer::new(),
            muzzle_tex: None,
            muzzle_anims: std::array::from_fn(|_| Vec::new()),
            muzzle_active: false,
            muzzle_dir: 0,
            muzzle_offset: [0.0, 0.0],
            muzzle_time_s: 0.0,
            pupil_state: PupilLook::Right,
            nene_knife_enabled: false,
            nene_state: NeneState::Default,
            blink_countdown: 3,
        }
    }
}

impl ABotSpeaker {
    fn load(
        gf_id: &str,
        stage_id: &str,
        gf: Option<&BaseCharacter>,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Self {
        let lower = gf_id.to_ascii_lowercase();
        let is_nene = lower.starts_with("nene");
        let is_otis = lower == "otis-speaker";
        let is_tankmen = is_otis || lower == "nene-tankmen";
        let is_pixel = lower == "nene-pixel";
        if (!is_nene && !is_otis) || is_pixel {
            return Self::default();
        }
        let is_dark = lower == "nene-dark" || stage_id.eq_ignore_ascii_case("spookyMansionErect");
        let abot_path = if is_dark {
            "characters/abot/dark/abotSystem"
        } else {
            "characters/abot/abotSystem"
        };
        let mut shared_assets = assets.clone();
        shared_assets.set_current_level(Some("shared"));

        let abot = LoadedAnimateProp::load(&shared_assets, gpu, abot_path)
            .or_else(|| LoadedAnimateProp::load(assets, gpu, abot_path));
        let eyes = LoadedAnimateProp::load(&shared_assets, gpu, "characters/abot/systemEyes")
            .or_else(|| LoadedAnimateProp::load(assets, gpu, "characters/abot/systemEyes"));
        let stereo_bg = upload_png_path(
            gpu,
            Some("stereoBG"),
            &shared_assets.image("characters/abot/stereoBG"),
        );

        let mut viz_tex = None;
        let mut viz_bars: [Vec<AtlasFrame>; 7] = std::array::from_fn(|_| Vec::new());
        let xml_path = shared_assets.sparrow_xml("characters/abot/aBotViz");
        let png_path = shared_assets.image("characters/abot/aBotViz");
        if let (Ok(xml), Some(tex_pair)) = (
            std::fs::read_to_string(&xml_path),
            upload_png_path(gpu, Some("aBotViz"), &png_path),
        ) {
            if let Ok(sparrow) = SparrowAtlas::from_xml(&xml) {
                for (i, bar) in viz_bars.iter_mut().enumerate() {
                    let mut prefix_buf = [0u8; 16];
                    let pfx_len = {
                        use std::io::Write as _;
                        let mut cur = std::io::Cursor::new(&mut prefix_buf[..]);
                        let _ = write!(cur, "viz{}0", i + 1);
                        cur.position() as usize
                    };
                    let pfx = std::str::from_utf8(&prefix_buf[..pfx_len]).unwrap_or("");
                    *bar = sparrow.frames_by_prefix(pfx).cloned().collect();
                }
                viz_tex = Some(tex_pair);
            }
        }

        let mut muzzle_tex = None;
        let mut muzzle_anims: [Vec<AtlasFrame>; 4] = std::array::from_fn(|_| Vec::new());
        if is_otis {
            let muzzle_key = "characters/otis/muzzle-flashes/otis_flashes";
            let m_xml_path = shared_assets.sparrow_xml(muzzle_key);
            let m_png_path = shared_assets.image(muzzle_key);
            if let (Ok(xml), Some(tex_pair)) = (
                std::fs::read_to_string(&m_xml_path)
                    .or_else(|_| std::fs::read_to_string(assets.sparrow_xml(muzzle_key))),
                upload_png_path(gpu, Some("otis_flashes"), &m_png_path).or_else(|| {
                    upload_png_path(gpu, Some("otis_flashes"), &assets.image(muzzle_key))
                }),
            ) {
                if let Ok(sparrow) = SparrowAtlas::from_xml(&xml) {
                    const PREFIXES: [&str; 4] = [
                        "shoot back0",
                        "shoot back low0",
                        "shoot forward0",
                        "shoot forward low0",
                    ];
                    for (slot, pfx) in muzzle_anims.iter_mut().zip(PREFIXES.iter()) {
                        *slot = sparrow.frames_by_prefix(pfx).cloned().collect();
                    }
                    muzzle_tex = Some(tex_pair);
                }
            }
        }

        let gf_z = gf.map_or(300, |g| g.z_index);
        let nene_knife_enabled = is_nene;
        Self {
            enabled: abot.is_some() || stereo_bg.is_some(),
            is_dark,
            is_otis,
            is_tankmen,
            gf_z,
            abot,
            eyes,
            stereo_bg,
            white_tex: gpu.white_texture(),
            viz_tex,
            viz_bars,
            viz_levels: [0; 7],
            analyzer: ABotSpectralAnalyzer::new(),
            muzzle_tex,
            muzzle_anims,
            muzzle_active: false,
            muzzle_dir: 0,
            muzzle_offset: [0.0, 0.0],
            muzzle_time_s: 0.0,
            pupil_state: PupilLook::Right,
            nene_knife_enabled,
            nene_state: NeneState::Default,
            blink_countdown: 3,
        }
    }

    fn reset(&mut self) {
        self.pupil_state = PupilLook::Right;
        self.nene_state = NeneState::Default;
        self.blink_countdown = 3;
        self.viz_levels = [0; 7];
        self.analyzer.reset();
        self.muzzle_active = false;
        self.muzzle_time_s = 0.0;
        if let Some(a) = &mut self.abot {
            a.play_from(1);
        }
        if let Some(e) = &mut self.eyes {
            e.play_from(17);
        }
    }

    fn trigger_muzzle_flash(&mut self, dir: usize) {
        if !self.is_otis || self.muzzle_tex.is_none() {
            return;
        }
        let d = dir.min(3);
        if self.muzzle_anims[d].is_empty() {
            return;
        }
        self.muzzle_dir = d;
        self.muzzle_offset = match d {
            0 => [950.0, 0.0],
            1 => [950.0, -50.0],
            2 => [-350.0, -50.0],
            _ => [-350.0, -100.0],
        };
        self.muzzle_time_s = 0.0;
        self.muzzle_active = true;
    }

    fn look_left(&mut self) {
        if self.pupil_state != PupilLook::Left {
            self.pupil_state = PupilLook::Left;
            if let Some(e) = &mut self.eyes {
                e.play_from(0);
            }
        }
    }

    fn look_right(&mut self) {
        if self.pupil_state != PupilLook::Right {
            self.pupil_state = PupilLook::Right;
            if let Some(e) = &mut self.eyes {
                e.play_from(17);
            }
        }
    }

    fn on_beat_hit(&mut self, beat: i32, gf: Option<&mut BaseCharacter>, rng: &mut ScriptRng) {
        if !self.enabled {
            return;
        }
        if let Some(a) = &mut self.abot {
            a.play_from(1);
        }
        if self.nene_knife_enabled {
            if let Some(gf) = gf {
                match self.nene_state {
                    NeneState::Default => {
                        gf.on_beat_hit(beat, false);
                    }
                    NeneState::PreRaise => {
                        gf.play_animation("danceLeft", false, false);
                        gf.has_danced = false;
                    }
                    NeneState::Ready => {
                        if self.blink_countdown <= 0 {
                            gf.play_animation("idleKnife", true, false);
                            self.blink_countdown = rng.range_i32(3, 7);
                        } else {
                            self.blink_countdown -= 1;
                        }
                    }
                    NeneState::Lower if gf.current_animation() != "lowerKnife" => {
                        gf.play_animation("lowerKnife", true, true);
                    }
                    _ => {}
                }
            }
        }
    }

    fn update(
        &mut self,
        elapsed_s: f32,
        song_pos_ms: f64,
        health: f32,
        train_passing: bool,
        gf: Option<&mut BaseCharacter>,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(a) = &mut self.abot {
            a.update_stopped_at(elapsed_s, None);
        }
        if let Some(e) = &mut self.eyes {
            let stop_at = match self.pupil_state {
                PupilLook::Left => Some(16),
                PupilLook::Right => Some(30),
            };
            e.update_stopped_at(elapsed_s, stop_at);
        }

        self.analyzer
            .analyze(song_pos_ms, elapsed_s, &mut self.viz_levels);

        if self.muzzle_active {
            self.muzzle_time_s += elapsed_s.max(0.0);
            let frame_idx = (self.muzzle_time_s * 24.0).floor() as usize;
            if frame_idx >= self.muzzle_anims[self.muzzle_dir].len() {
                self.muzzle_active = false;
            }
        }

        if self.nene_knife_enabled {
            if let Some(gf) = gf {
                let finished = gf.is_animation_finished();
                let anim = gf.current_animation().to_owned();
                let frame = gf.active_frame_snapshot().map_or(0, |(_, f, _)| f);
                const VULTURE_THRESHOLD: f32 = 0.5;
                match self.nene_state {
                    NeneState::Default => {
                        if health <= VULTURE_THRESHOLD {
                            self.nene_state = NeneState::PreRaise;
                        }
                        if train_passing {
                            self.nene_state = NeneState::HairBlowing;
                            gf.play_animation("hairBlowNormal", true, true);
                        }
                    }
                    NeneState::PreRaise => {
                        if health > VULTURE_THRESHOLD {
                            self.nene_state = NeneState::Default;
                        } else if anim == "danceLeft" && frame >= 13 {
                            self.nene_state = NeneState::Raise;
                            gf.play_animation("raiseKnife", true, true);
                        }
                        if train_passing {
                            self.nene_state = NeneState::HairBlowing;
                            gf.play_animation("hairBlowNormal", true, true);
                        }
                    }
                    NeneState::Raise => {
                        if finished && anim == "raiseKnife" {
                            self.nene_state = NeneState::Ready;
                        }
                        if train_passing {
                            self.nene_state = NeneState::HairBlowingRaise;
                            gf.play_animation("hairBlowKnife", true, true);
                        }
                    }
                    NeneState::Ready => {
                        if health > VULTURE_THRESHOLD {
                            self.nene_state = NeneState::Lower;
                        }
                        if train_passing {
                            self.nene_state = NeneState::HairBlowingRaise;
                            gf.play_animation("hairBlowKnife", true, true);
                        }
                    }
                    NeneState::Lower => {
                        if finished && anim == "lowerKnife" {
                            self.nene_state = NeneState::Default;
                        }
                        if train_passing {
                            self.nene_state = NeneState::HairBlowing;
                            gf.play_animation("hairBlowNormal", true, true);
                        }
                    }
                    NeneState::HairBlowing => {
                        if !train_passing {
                            self.nene_state = NeneState::HairFalling;
                            gf.play_animation("hairFallNormal", true, true);
                        } else if finished {
                            gf.play_animation("hairBlowNormal", true, true);
                        }
                    }
                    NeneState::HairFalling => {
                        if finished {
                            self.nene_state = NeneState::Default;
                        }
                    }
                    NeneState::HairBlowingRaise => {
                        if !train_passing {
                            self.nene_state = NeneState::HairFallingRaise;
                            gf.play_animation("hairFallKnife", true, true);
                        } else if finished {
                            gf.play_animation("hairBlowKnife", true, true);
                        }
                    }
                    NeneState::HairFallingRaise => {
                        if finished {
                            self.nene_state = NeneState::Ready;
                        }
                    }
                }
            }
        }
    }

    fn base_pos(&self, gf: &BaseCharacter) -> [f32; 2] {
        // otis-speaker.hxc: +170, +455; nene.hxc: -95, +384
        let [dx, dy] = if self.is_otis {
            [170.0, 455.0]
        } else {
            [-95.0, 384.0]
        };
        [
            gf.position[0] + dx + gf.global_offsets[0] * gf.scale,
            gf.position[1] + dy + gf.global_offsets[1] * gf.scale,
        ]
    }

    fn draw_back(&self, batcher: &mut SpriteBatcher, gf: &BaseCharacter, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.enabled || gf.alpha <= 0.0 {
            return;
        }
        let ab = self.base_pos(gf);
        let scroll = gf.scroll_factor;
        let alpha = gf.alpha;

        let abot_adjust = if self.is_tankmen {
            Some([-40.0, -20.0, -40.0, -25.0])
        } else {
            None
        };
        let viz_adjust = if self.is_tankmen {
            Some([-30.0, -10.0, -12.0, 0.0])
        } else if self.is_dark {
            Some([-26.0, -45.0, -12.0, 0.0])
        } else {
            None
        };

        let eye_whites_color = if self.is_dark {
            let t = alpha.clamp(0.0, 1.0);
            [
                1.0 + (0x6F as f32 / 255.0 - 1.0) * t,
                1.0 + (0x96 as f32 / 255.0 - 1.0) * t,
                1.0 + (0xCE as f32 / 255.0 - 1.0) * t,
                alpha,
            ]
        } else {
            [1.0, 1.0, 1.0, alpha]
        };
        let stereo_color = if self.is_dark {
            [
                0x61 as f32 / 255.0,
                0x67 as f32 / 255.0,
                0x85 as f32 / 255.0,
                alpha,
            ]
        } else {
            [1.0, 1.0, 1.0, alpha]
        };
        let shell_color = [1.0, 1.0, 1.0, alpha];

        // eyeWhites (`z = gf_z - 20`)
        let (ew_pos, ew_sz) = apply_camera_transform([ab[0] + 40.0, ab[1] + 250.0], [160.0, 60.0], scroll, cam_pos, cam_zoom);
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Linear,
            SpriteInstance::new(ew_pos, ew_sz).with_color(eye_whites_color),
        );

        // stereoBG (`z = gf_z - 18`)
        if let Some((tex, sz)) = self.stereo_bg {
            let (p, s) = apply_camera_transform([ab[0] + 150.0, ab[1] + 30.0], sz, scroll, cam_pos, cam_zoom);
            let mut inst = SpriteInstance::new(p, s).with_color(stereo_color);
            if let Some([h, s_val, b, c]) = abot_adjust {
                inst = inst.with_adjust_color(h, s_val, b, c);
            }
            batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
        }

        // eyes (`z = gf_z - 15`)
        if let Some(e) = &self.eyes {
            e.draw(batcher, [ab[0] + 50.0, ab[1] + 238.0], gf.scale, scroll, cam_pos, cam_zoom, shell_color, abot_adjust);
        }

        // aBotViz 7 bars (`z = gf_z - 11`)
        if let Some((viz_tex, viz_tex_sz)) = self.viz_tex {
            const DX: [f32; 7] = [0.0, 59.0, 56.0, 66.0, 54.0, 52.0, 51.0];
            const DY: [f32; 7] = [0.0, -8.0, -3.5, -0.4, 0.5, 4.7, 7.0];
            let mut bx = ab[0] + 207.0;
            let mut by = ab[1] + 84.0;
            for i in 0..7usize {
                bx += DX[i];
                by += DY[i];
                let level = self.viz_levels[i];
                if level == 0 {
                    continue;
                }
                let frames = &self.viz_bars[i];
                if frames.is_empty() {
                    continue;
                }
                // Matches ABotVis.hx line 160 (`abs(animFrame - 5)`): higher loudness = lower frame index
                let frame_idx = (6u8.saturating_sub(level) as usize).min(frames.len() - 1);
                let frame = &frames[frame_idx];
                let (p, s) = apply_camera_transform(
                    [bx + (-frame.frame_x), by + (-frame.frame_y)],
                    frame.upright_trimmed_size(),
                    scroll,
                    cam_pos,
                    cam_zoom,
                );
                let mut flags = 0u32;
                if frame.rotated {
                    flags |= SPRITE_FLAG_ROTATED_90_CCW;
                }
                let mut inst = SpriteInstance::new(p, s)
                    .with_uv_rect(frame.uv_bounds_inset(viz_tex_sz[0], viz_tex_sz[1], 0.5))
                    .with_color(shell_color)
                    .with_flags(flags);
                if let Some([h, s_val, b, c]) = viz_adjust {
                    inst = inst.with_adjust_color(h, s_val, b, c);
                }
                batcher.push(viz_tex, BlendMode::Normal, FilterMode::Linear, inst);
            }
        }

        // abot speaker shell (`z = gf_z - 10`)
        if let Some(a) = &self.abot {
            a.draw(batcher, ab, gf.scale, scroll, cam_pos, cam_zoom, shell_color, abot_adjust);
        }

        // otis-speaker muzzleFlash (`z = gf_z - 1`, behind gf)
        if self.muzzle_active {
            if let Some((tex, tex_sz)) = self.muzzle_tex {
                let frames = &self.muzzle_anims[self.muzzle_dir];
                let frame_idx = (self.muzzle_time_s * 24.0).floor() as usize;
                if let Some(frame) = frames.get(frame_idx) {
                    let trimmed = frame.upright_trimmed_size();
                    if trimmed[0] > 0.0 && trimmed[1] > 0.0 {
                        let draw_x = gf.position[0] + self.muzzle_offset[0] + (-frame.frame_x);
                        let draw_y = gf.position[1] + self.muzzle_offset[1] + (-frame.frame_y);
                        let (p, s) = apply_camera_transform(
                            [draw_x, draw_y],
                            trimmed,
                            scroll,
                            cam_pos,
                            cam_zoom,
                        );
                        let mut flags = 0u32;
                        if frame.rotated {
                            flags |= SPRITE_FLAG_ROTATED_90_CCW;
                        }
                        let blend = if frame_idx <= 1 {
                            BlendMode::Add
                        } else {
                            BlendMode::Normal
                        };
                        batcher.push(
                            tex,
                            blend,
                            FilterMode::Linear,
                            SpriteInstance::new(p, s)
                                .with_uv_rect(frame.uv_bounds_inset(tex_sz[0], tex_sz[1], 0.5))
                                .with_color(shell_color)
                                .with_flags(flags),
                        );
                    }
                }
            }
        }
    }
}

// ============================================================================
// 4. Week 7 Tankman Runner Group (`TankmanSpriteGroup.hxc` + `TankmanSprite.hxc`)
// ============================================================================

#[derive(Debug, Clone)]
struct TankRunner {
    active: bool,
    visible: bool,
    strum_time_ms: f64,
    going_right: bool,
    ending_offset: f32,
    run_speed: f32,
    scale: f32,
    x: f32,
    y: f32,
    shot: bool,
    shot_variant: usize,
    anim_time_s: f32,
    flicker_timer_s: f32,
    flickering: bool,
}

impl Default for TankRunner {
    fn default() -> Self {
        Self {
            active: false,
            visible: true,
            strum_time_ms: 0.0,
            going_right: false,
            ending_offset: 0.0,
            run_speed: 0.8,
            scale: 1.0,
            x: 9999.0,
            y: 250.0,
            shot: false,
            shot_variant: 0,
            anim_time_s: 0.0,
            flicker_timer_s: 0.0,
            flickering: false,
        }
    }
}

#[derive(Debug)]
struct TankmanRunners {
    enabled: bool,
    is_erect: bool,
    z_index: i32,
    texture: Option<TextureHandle>,
    tex_size: [f32; 2],
    run_frames: Vec<AtlasFrame>,
    shot_frames: [Vec<AtlasFrame>; 2],
    timemap: Vec<(f64, bool)>,
    next_spawn: usize,
    pool: [TankRunner; 4],
}

impl Default for TankmanRunners {
    fn default() -> Self {
        Self {
            enabled: false,
            is_erect: false,
            z_index: 30,
            texture: None,
            tex_size: [1.0, 1.0],
            run_frames: Vec::new(),
            shot_frames: [Vec::new(), Vec::new()],
            timemap: Vec::new(),
            next_spawn: 0,
            pool: std::array::from_fn(|_| TankRunner::default()),
        }
    }
}

impl TankmanRunners {
    fn load(
        song_id: &str,
        stage_id: &str,
        picospeaker_notes: &[SongNoteData],
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        rng: &mut ScriptRng,
    ) -> Self {
        let stage_lower = stage_id.to_ascii_lowercase();
        if !song_id.eq_ignore_ascii_case("stress")
            || (stage_lower != "tankmanbattlefield" && stage_lower != "tankmanbattlefielderect")
            || picospeaker_notes.is_empty()
        {
            return Self::default();
        }
        let is_erect = stage_lower == "tankmanbattlefielderect";
        let xml_path = assets.sparrow_xml("tankmanKilled1");
        let png_path = assets.image("tankmanKilled1");
        let (Ok(xml), Some((tex, tex_size))) = (
            std::fs::read_to_string(&xml_path),
            upload_png_path(gpu, Some("tankmanKilled1"), &png_path),
        ) else {
            return Self::default();
        };
        let Ok(sparrow) = SparrowAtlas::from_xml(&xml) else {
            return Self::default();
        };

        let mut sorted: Vec<&SongNoteData> = picospeaker_notes.iter().collect();
        sorted.sort_by(|a, b| a.time.total_cmp(&b.time));
        let mut timemap = Vec::new();
        for note in sorted {
            if rng.range_i32(0, 15) == 0 {
                let going_right = note.data != 2 && note.data != 3;
                timemap.push((note.time, going_right));
            }
        }

        Self {
            enabled: true,
            is_erect,
            z_index: 30,
            texture: Some(tex),
            tex_size,
            run_frames: sparrow.frames_by_prefix("tankman running").cloned().collect(),
            shot_frames: [
                sparrow.frames_by_prefix("John Shot 1").cloned().collect(),
                sparrow.frames_by_prefix("John Shot 2").cloned().collect(),
            ],
            timemap,
            next_spawn: 0,
            pool: std::array::from_fn(|_| TankRunner::default()),
        }
    }

    fn reset(&mut self) {
        self.next_spawn = 0;
        for r in &mut self.pool {
            r.active = false;
        }
    }

    fn update(&mut self, elapsed_s: f32, song_pos_ms: f64, rng: &mut ScriptRng) {
        if !self.enabled {
            return;
        }
        let cutoff = song_pos_ms + 3000.0;
        while self.next_spawn < self.timemap.len() && self.timemap[self.next_spawn].0 <= cutoff {
            let (strum_time, going_right) = self.timemap[self.next_spawn];
            self.next_spawn += 1;
            if let Some(slot) = self.pool.iter_mut().find(|r| !r.active) {
                *slot = TankRunner {
                    active: true,
                    visible: true,
                    strum_time_ms: strum_time,
                    going_right,
                    ending_offset: rng.range_f32(50.0, 200.0),
                    run_speed: rng.range_f32(0.6, 1.0),
                    scale: if self.is_erect { 1.10 } else { 1.0 },
                    x: 9999.0,
                    y: if self.is_erect {
                        350.0
                    } else {
                        (200 + rng.range_i32(50, 100)) as f32
                    },
                    shot: false,
                    shot_variant: (rng.next_u32() & 1) as usize,
                    anim_time_s: rng.range_f32(0.0, 0.5),
                    flicker_timer_s: 0.0,
                    flickering: false,
                };
            }
        }

        for r in &mut self.pool {
            if !r.active {
                continue;
            }
            r.anim_time_s += elapsed_s.max(0.0);
            if !r.shot && song_pos_ms >= r.strum_time_ms {
                r.shot = true;
                r.anim_time_s = 0.0;
            }
            if !r.shot {
                let run_factor = ((song_pos_ms - r.strum_time_ms) as f32) * r.run_speed;
                r.x = if !r.going_right {
                    (1280.0 * 0.02 - r.ending_offset) + run_factor
                } else {
                    (1280.0 * 0.74 + r.ending_offset) - run_factor
                };
            } else {
                let frame_num = (r.anim_time_s * 24.0).floor() as usize;
                if frame_num >= 10 {
                    r.flickering = true;
                }
                if r.flickering {
                    r.flicker_timer_s += elapsed_s.max(0.0);
                    if r.flicker_timer_s < 0.3 {
                        let flick = (r.flicker_timer_s * 10.0) as i32;
                        r.visible = (flick & 1) == 0;
                    } else {
                        let flick = ((r.flicker_timer_s - 0.3) * 20.0) as i32;
                        r.visible = (flick & 1) == 0;
                    }
                    if r.flicker_timer_s >= 0.6 {
                        r.active = false;
                    }
                }
            }
        }
    }

    fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.enabled {
            return;
        }
        let Some(tex) = self.texture else {
            return;
        };
        const ORIGIN: [f32; 2] = [252.5, 239.0];
        for r in &self.pool {
            if !r.active || !r.visible {
                continue;
            }
            let frames = if r.shot {
                &self.shot_frames[r.shot_variant]
            } else {
                &self.run_frames
            };
            if frames.is_empty() {
                continue;
            }
            let idx = (r.anim_time_s * 24.0).floor() as usize;
            let frame = if r.shot {
                &frames[idx.min(frames.len() - 1)]
            } else {
                &frames[idx % frames.len()]
            };
            let src = frame.source_size();
            let trimmed = frame.upright_trimmed_size();
            let flip_x = !r.going_right;
            let trim = compute_flipped_trim_offset(
                src,
                trimmed,
                [frame.frame_x, frame.frame_y],
                flip_x,
                false,
            );
            let offset = if r.shot {
                [300.0, 200.0]
            } else {
                [151.5, 143.4]
            };
            let draw_x =
                r.x - offset[0] + ORIGIN[0] * (1.0 - r.scale) + trim[0] * r.scale;
            let draw_y =
                r.y - offset[1] + ORIGIN[1] * (1.0 - r.scale) + trim[1] * r.scale;
            let (pos, size) = apply_camera_transform(
                [draw_x, draw_y],
                [trimmed[0] * r.scale, trimmed[1] * r.scale],
                [1.0, 1.0],
                cam_pos,
                cam_zoom,
            );
            let mut flags = 0u32;
            if frame.rotated {
                flags |= SPRITE_FLAG_ROTATED_90_CCW;
            }
            if flip_x ^ frame.flip_x {
                flags |= SPRITE_FLAG_FLIP_X;
            }
            let mut inst = SpriteInstance::new(pos, size)
                .with_uv_rect(frame.uv_bounds_inset(self.tex_size[0], self.tex_size[1], 1.0))
                .with_flags(flags);
            if self.is_erect {
                inst = inst.with_adjust_color(-38.0, -20.0, -46.0, -25.0);
            }
            batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
        }
    }
}

// ============================================================================
// 5. Stage Dynamic Behaviors (`spookyMansion`, `phillyTrain`, `limoRide`, `school`, `tankmanBattlefield`, `phillyStreets`)
// ============================================================================

#[derive(Debug, Clone, Default)]
struct QuadCarTween {
    active: bool,
    elapsed_s: f32,
    duration_s: f32,
    start_delay_s: f32,
    p0: [f32; 2],
    p1: [f32; 2],
    p2: [f32; 2],
    rot_start: f32,
    rot_end: f32,
    stopping_at_lights: bool,
}

impl QuadCarTween {
    fn eval(&mut self, elapsed_s: f32, sprite_x: &mut f32, sprite_y: &mut f32, sprite_angle: &mut f32) -> (bool, bool) {
        // Returns (still_active, finished_this_frame)
        if !self.active {
            return (false, false);
        }
        self.elapsed_s += elapsed_s.max(0.0);
        if self.elapsed_s < self.start_delay_s {
            return (true, false);
        }
        let active_t = self.elapsed_s - self.start_delay_s;
        let raw_t = (active_t / self.duration_s.max(0.01)).clamp(0.0, 1.0);
        // cubeOut when stopping at lights, sineIn when pulling away, linear otherwise
        let t = if self.stopping_at_lights {
            let u = 1.0 - raw_t;
            1.0 - u * u * u
        } else if self.start_delay_s > 0.0 {
            1.0 - (raw_t * PI * 0.5).cos()
        } else {
            raw_t
        };
        let u = 1.0 - t;
        *sprite_x = u * u * self.p0[0] + 2.0 * u * t * self.p1[0] + t * t * self.p2[0];
        *sprite_y = u * u * self.p0[1] + 2.0 * u * t * self.p1[1] + t * t * self.p2[1];
        *sprite_angle = self.rot_start + (self.rot_end - self.rot_start) * t;
        if raw_t >= 1.0 {
            self.active = false;
            return (false, true);
        }
        (true, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CanState {
    #[default]
    Inactive,
    Arcing,
    Shot,
    HitPico,
}

#[derive(Debug, Clone, Copy, Default)]
struct SpraycanSlot {
    state: CanState,
    pos: [f32; 2],
    anim_time_s: f32,
}

#[derive(Debug)]
struct SpraycanPool {
    enabled: bool,
    atlas: Option<AnimateAtlas>,
    textures: Vec<(TextureHandle, [f32; 2])>,
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    parts_bufs: [Vec<EvaluatedPart>; 4],
    slots: [SpraycanSlot; 4],
    gun_cocked_timer_s: f32,
    darken_timer_s: Option<f32>,
}

impl Default for SpraycanPool {
    fn default() -> Self {
        Self {
            enabled: false,
            atlas: None,
            textures: Vec::new(),
            bounds_origin: [0.0, 0.0],
            bounds_size: [1.0, 1.0],
            parts_bufs: Default::default(),
            slots: [SpraycanSlot::default(); 4],
            gun_cocked_timer_s: 0.0,
            darken_timer_s: None,
        }
    }
}

impl SpraycanPool {
    fn load(song_id: &str, assets: &AssetLocator, gpu: &mut GpuContext) -> Self {
        if !song_id.eq_ignore_ascii_case("2hot") {
            return Self::default();
        }
        let Some(LoadedAtlasSource::Animate {
            atlas,
            textures,
            bounds_origin,
            bounds_size,
            ..
        }) = load_animate_source(assets, gpu, "spraycanAtlas", None)
        else {
            return Self::default();
        };
        Self {
            enabled: true,
            atlas: Some(atlas),
            textures,
            bounds_origin,
            bounds_size,
            parts_bufs: [
                Vec::with_capacity(24),
                Vec::with_capacity(24),
                Vec::with_capacity(24),
                Vec::with_capacity(24),
            ],
            slots: [SpraycanSlot::default(); 4],
            gun_cocked_timer_s: 0.0,
            darken_timer_s: None,
        }
    }

    fn reset(&mut self) {
        self.slots = [SpraycanSlot::default(); 4];
        self.gun_cocked_timer_s = 0.0;
        self.darken_timer_s = None;
    }

    fn spawn(&mut self, stage: Option<&Stage>) {
        if !self.enabled {
            return;
        }
        let pile_pos = stage
            .and_then(|st| st.get_named_prop("spraycanPile"))
            .map_or([890.0, 740.0], |p| p.position);
        for slot in &mut self.slots {
            if slot.state == CanState::Inactive {
                slot.state = CanState::Arcing;
                slot.pos = [pile_pos[0] - 10.0, pile_pos[1] - 550.0];
                slot.anim_time_s = 0.0;
                break;
            }
        }
    }

    fn shoot(&mut self) -> bool {
        if !self.enabled || self.gun_cocked_timer_s <= 0.0 {
            return false;
        }
        self.gun_cocked_timer_s = 0.0;
        for slot in &mut self.slots {
            if slot.state == CanState::Arcing {
                slot.state = CanState::Shot;
                slot.anim_time_s = 0.0;
                self.darken_timer_s = Some(0.0);
                return true;
            }
        }
        false
    }

    fn update(&mut self, elapsed_s: f32, mut stage: Option<&mut Stage>) {
        if !self.enabled {
            return;
        }
        let dt = elapsed_s.max(0.0);
        if self.gun_cocked_timer_s > 0.0 {
            self.gun_cocked_timer_s = (self.gun_cocked_timer_s - dt).max(0.0);
        }
        let Some(atlas) = &self.atlas else {
            return;
        };
        let fps = atlas.frame_rate().max(1.0);
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == CanState::Inactive {
                self.parts_bufs[idx].clear();
                continue;
            }
            slot.anim_time_s += dt;
            let label = match slot.state {
                CanState::Inactive => continue,
                CanState::Arcing => "Can Start",
                CanState::Shot => "Can Shot",
                CanState::HitPico => "Hit Pico",
            };
            let dur = atlas.label_span(label).map_or(1, |s| s.duration.max(1));
            let frame = (slot.anim_time_s * fps).floor() as u32;
            if frame >= dur {
                if slot.state == CanState::Arcing {
                    slot.state = CanState::HitPico;
                    slot.anim_time_s = 0.0;
                    atlas.evaluate_label("Hit Pico", 0, &mut self.parts_bufs[idx]);
                } else {
                    slot.state = CanState::Inactive;
                    self.parts_bufs[idx].clear();
                }
            } else {
                atlas.evaluate_label(label, frame, &mut self.parts_bufs[idx]);
            }
        }

        if let Some(t) = &mut self.darken_timer_s {
            *t += dt;
            let c = if *t < 1.0 / 24.0 {
                0.067
            } else {
                let u = ((*t - 1.0 / 24.0) / 1.4).clamp(0.0, 1.0);
                0.133 + (1.0 - 0.133) * u
            };
            if let Some(st) = stage.as_deref_mut() {
                for prop in &mut st.props {
                    prop.color = [c, c, c, prop.color[3]];
                }
            }
            if *t >= 1.45 {
                if let Some(st) = stage {
                    for prop in &mut st.props {
                        prop.color = [1.0, 1.0, 1.0, prop.color[3]];
                    }
                }
                self.darken_timer_s = None;
            }
        }
    }

    fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.enabled || self.atlas.is_none() {
            return;
        }
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.state == CanState::Inactive || self.parts_bufs[idx].is_empty() {
                continue;
            }
            let params = AtlasDrawParams {
                pos: slot.pos,
                scale: [1.0, 1.0],
                anim_offsets: [0.0, 0.0],
                global_offsets: [0.0, 0.0],
                scroll_factor: [1.0, 1.0],
                cam_pos,
                cam_zoom,
                angle_deg: 0.0,
                flip_x: false,
                flip_y: false,
                color: [1.0, 1.0, 1.0, 1.0],
                alpha: 1.0,
                blend: BlendMode::Normal,
                is_pixel: false,
                adjust_color: None,
                wiggle: None,
                ref_unscaled_size: None,
            };
            draw_animate_parts(
                batcher,
                &self.textures,
                &self.parts_bufs[idx],
                self.bounds_origin,
                self.bounds_size,
                None,
                &params,
                false,
                false,
            );
        }
    }
}

#[derive(Debug)]
enum StageScript {
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

fn place_extra_sserafim_char(
    mut ch: BaseCharacter,
    x: f32,
    y: f32,
    scroll: [f32; 2],
    hsbc: [f32; 4],
) -> Box<BaseCharacter> {
    let origin = ch.character_origin();
    ch.position = [x - origin[0], y - origin[1]];
    ch.original_position = ch.position;
    ch.scroll_factor = scroll;
    ch.z_index = 1000;
    ch.adjust_color = Some(hsbc);
    ch.reset_character(false);
    Box::new(ch)
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
    stage_script: StageScript,
    spirit_trail: SpiritTrail,
    speaker_shooter: SpeakerShooter,
    abot: ABotSpeaker,
    runners: TankmanRunners,
    spraycans: SpraycanPool,
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
    /// Initializes native script controllers from song/stage/character IDs and loads
    /// any auxiliary script textures/atlases upfront.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        song_id: &str,
        stage_id: &str,
        dad_id: &str,
        gf_id: &str,
        gf: Option<&BaseCharacter>,
        picospeaker_notes: &[SongNoteData],
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        sfx: ScriptSfxSlots,
    ) -> Self {
        let mut rng = ScriptRng(0x5A17_C0DE);
        let stage_lower = stage_id.to_ascii_lowercase();
        let stage_script = if stage_lower == "spookymansion" || stage_lower == "spookymansionerect"
        {
            StageScript::SpookyMansion {
                is_erect: stage_lower == "spookymansionerect",
                lightning_strike_beat: 0,
                lightning_strike_offset: 8,
                flash_timer_s: None,
            }
        } else if stage_lower == "phillytrain" || stage_lower == "phillytrainerect" {
            StageScript::PhillyTrain {
                light_alpha: 1.0,
                cur_light: 0,
                train_moving: false,
                train_started: false,
                train_finishing: false,
                train_cars: 8,
                train_cooldown: 0,
                train_timer_s: 0.0,
                train_sound_ms: 0.0,
            }
        } else if stage_lower == "phillyblazin" {
            let sky_blur_tex = upload_png_path(
                gpu,
                Some("phillyBlazin/skyBlur"),
                &assets.image("phillyBlazin/skyBlur"),
            );
            StageScript::PhillyBlazin {
                sky_blur_tex,
                sky_scroll_x: 0.0,
                rain_time_scale: 1.0,
                lightning_timer_s: 3.0,
                lightning_fade_s: 0.0,
                lightning_fading: false,
            }
        } else if stage_lower == "limoride" {
            StageScript::LimoRide {
                fast_car_can_drive: true,
                fast_car_active: false,
                fast_car_timer_s: 0.0,
                fast_car_vx: 0.0,
            }
        } else if stage_lower == "school" || stage_lower == "schoolerect" {
            StageScript::School
        } else if stage_lower == "schoolevil" || stage_lower == "schoolevilerect" {
            StageScript::SchoolEvil { time_s: 0.0 }
        } else if stage_lower == "tankmanbattlefield" || stage_lower == "tankmanbattlefielderect" {
            let is_erect = stage_lower == "tankmanbattlefielderect";
            let clouds_tex = if !is_erect {
                upload_png_path(gpu, Some("tankClouds"), &assets.image("tankClouds"))
            } else {
                None
            };
            StageScript::TankmanBattlefield {
                is_erect,
                clouds_tex,
                clouds_x: -1100.0,
                tank_angle: rng.range_i32(-90, 45) as f32,
                tank_speed: rng.range_f32(5.0, 7.0),
            }
        } else if stage_lower == "phillystreets" || stage_lower == "phillystreetserect" {
            let sky_tex = upload_png_path(
                gpu,
                Some("phillySkybox"),
                &assets.image("phillyStreets/phillySkybox"),
            );
            StageScript::PhillyStreets {
                sky_tex,
                sky_scroll_x: 0.0,
                lights_stop: false,
                last_change: 0,
                change_interval: 8,
                car_waiting: false,
                car_interruptable: true,
                car2_interruptable: true,
                car1: QuadCarTween::default(),
                car2: QuadCarTween::default(),
            }
        } else if stage_lower == "sserafim" {
            let yunjin = BaseCharacter::load_by_id("sserafim-yunjin", CharacterKind::Other, assets, gpu)
                .map(|c| place_extra_sserafim_char(c, -621.0, 154.0, [0.95, 0.95], [0.0; 4]));
            let chaewon = BaseCharacter::load_by_id("sserafim-chaewon", CharacterKind::Other, assets, gpu)
                .map(|c| place_extra_sserafim_char(c, 687.0, 98.0, [0.95, 0.95], [0.0; 4]));
            let eunchae = BaseCharacter::load_by_id("sserafim-eunchae", CharacterKind::Other, assets, gpu)
                .map(|c| place_extra_sserafim_char(c, 770.0, 675.0, [0.97, 0.97], [0.0; 4]));
            let dust_mid_tex = upload_png_path(gpu, Some("dustMid"), &assets.image("dust/dustMid"));
            let dust_back_tex = upload_png_path(gpu, Some("dustBack"), &assets.image("dust/dustBack"));
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                dust_mid_tex,
                dust_back_tex,
                dust_time_s: 0.0,
                visible: [true, false, false, false, false, false],
                singing: [false; 6],
                lights_enabled: false,
                light_colors: [[1.0; 4]; 8],
                light_durs: [0.5; 8],
                light_ints: [0.8; 8],
                light_count: 0,
                light_idx: 0,
                pulse_alpha: 0.0,
                pulse_dur: 0.5,
                truck_light_alpha: 0.0,
                truck_light_dur: 0.5,
                cover_alpha: 0.0,
                dark_amount: 0.0,
                dark_start: 0.0,
                dark_target: 0.0,
                dark_elapsed: 0.0,
                dark_dur: 0.0,
                kick_waiting_door: false,
            }
        } else {
            StageScript::None
        };

        let spirit_trail = SpiritTrail {
            enabled: dad_id.eq_ignore_ascii_case("spirit"),
            ..SpiritTrail::default()
        };
        let speaker_shooter = SpeakerShooter::new(gf_id, picospeaker_notes);
        let abot = ABotSpeaker::load(gf_id, stage_id, gf, assets, gpu);
        let runners = TankmanRunners::load(song_id, stage_id, picospeaker_notes, assets, gpu, &mut rng);
        let spraycans = SpraycanPool::load(song_id, assets, gpu);

        let rain_enabled = stage_lower == "phillystreetserect"
            || stage_lower == "phillyblazin"
            || stage_lower == "spookymansionerect";
        let (rain_scale, rain_start_intensity, rain_end_intensity) = if stage_lower == "phillyblazin"
        {
            (3.6, 0.5, 0.5)
        } else if stage_lower == "spookymansionerect" {
            (7.2, 0.4, 0.4)
        } else if stage_lower == "phillystreetserect" {
            let song_lower = song_id.to_ascii_lowercase();
            if song_lower == "darnell" {
                (3.6, 0.002, 0.01)
            } else if song_lower == "lit-up" {
                (3.6, 0.01, 0.02)
            } else if song_lower == "2hot" {
                (3.6, 0.02, 0.04)
            } else {
                (3.6, 0.01, 0.02)
            }
        } else {
            (3.6, 0.0, 0.0)
        };

        Self {
            song_id: song_id.to_owned(),
            stage_id_lower: stage_lower,
            stage_script,
            spirit_trail,
            speaker_shooter,
            abot,
            runners,
            spraycans,
            sfx,
            rng,
            rain_enabled,
            rain_time: 1.0,
            rain_scale,
            rain_intensity: rain_start_intensity,
            rain_start_intensity,
            rain_end_intensity,
            rain_duration_ms: 180_000.0,
            rain_color: [0.659, 0.678, 0.710],
        }
    }

    /// Minimal empty host for tests & headless scenes without a loaded stage script.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            song_id: String::new(),
            stage_id_lower: String::new(),
            stage_script: StageScript::None,
            spirit_trail: SpiritTrail::default(),
            speaker_shooter: SpeakerShooter::default(),
            abot: ABotSpeaker::default(),
            runners: TankmanRunners::default(),
            spraycans: SpraycanPool::default(),
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

    /// True for stages that suppress standard combo popups (`sserafim`).
    #[must_use]
    pub fn hides_popups(&self) -> bool {
        self.stage_id_lower == "sserafim"
    }

    /// Applies one-time initial stage setup & Erect `AdjustColor` shaders after placement.
    pub fn on_stage_ready(
        &mut self,
        mut stage: Option<&mut Stage>,
        mut bf: Option<&mut BaseCharacter>,
        mut dad: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) {
        apply_erect_adjust_color(
            &self.stage_id_lower,
            stage.as_deref_mut(),
            bf.as_deref_mut(),
            dad.as_deref_mut(),
            gf.as_deref_mut(),
        );
        match &mut self.stage_script {
            StageScript::SpookyMansion { is_erect: true, .. } => {
                if let Some(st) = stage {
                    for nm in ["bgLight", "stairsLight"] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = 0.0;
                        }
                    }
                }
            }
            StageScript::PhillyBlazin { .. } => {
                if let Some(st) = stage {
                    for (nm, blend) in [
                        ("skyAdditive", BlendMode::Add),
                        ("lightning", BlendMode::Normal),
                        ("foregroundMultiply", BlendMode::Multiply),
                        ("additionalLighten", BlendMode::Add),
                    ] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.visible = false;
                            p.blend = blend;
                        }
                    }
                }
                if let Some(b) = bf {
                    b.color = [0.871, 0.871, 0.871, 1.0];
                }
                if let Some(d) = dad {
                    d.color = [0.871, 0.871, 0.871, 1.0];
                }
                if let Some(g) = gf {
                    g.color = [0.533, 0.533, 0.533, 1.0];
                }
            }
            StageScript::School => {
                if self.song_id.eq_ignore_ascii_case("roses") {
                    if let Some(st) = stage {
                        if let Some(freaks) = st.get_named_prop_mut("freaks") {
                            if let Some(bop) = &mut freaks.bopper {
                                bop.idle_suffix.clear();
                                bop.idle_suffix.push_str("-scared");
                                bop.dance(true);
                            }
                        }
                    }
                }
            }
            StageScript::TankmanBattlefield { is_erect: true, .. } => {
                if let Some(st) = stage {
                    if let Some(bricks) = st.get_named_prop_mut("tankBricks") {
                        bricks.position = [445.0, 774.0];
                    }
                }
            }
            StageScript::PhillyTrain { .. } => {
                if let Some(st) = stage {
                    if let Some(train) = st.get_named_prop_mut("train") {
                        train.position[0] = 1480.0;
                    }
                }
            }
            StageScript::LimoRide { .. } => {
                if let Some(st) = stage {
                    if let Some(car) = st.get_named_prop_mut("fastCar") {
                        car.position = [-12600.0, 360.0];
                    }
                }
            }
            StageScript::PhillyStreets { .. } => {
                if let Some(st) = stage {
                    if let Some(traffic) = st.get_named_prop_mut("phillyTraffic") {
                        if let Some(bop) = &mut traffic.bopper {
                            bop.play_animation("togreen", true);
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                visible,
                ..
            } => {
                if let Some(st) = stage {
                    for nm in [
                        "solidCover",
                        "backTablesCutscene",
                        "burgerCutscene",
                        "backLightColor",
                        "backLightWhite",
                        "truckLight1",
                        "truckLight2",
                        "truckDoor",
                    ] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = 0.0;
                        }
                    }
                }
                if let Some(y) = yunjin.as_deref_mut() {
                    y.visible = visible[0];
                }
                if let Some(d) = dad {
                    d.visible = visible[1];
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.visible = visible[2];
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.visible = visible[3];
                }
                if let Some(b) = bf {
                    b.visible = visible[4];
                }
            }
            _ => {}
        }
    }

    /// Returns `true` when the active stage/character script manages GF's beat dancing directly
    /// (e.g., Nene's knife state machine).
    #[must_use]
    pub fn owns_gf_dance(&self) -> bool {
        self.abot.nene_knife_enabled
    }

    /// Dispatches song chart custom events (such as `sserafimShow`, `sserafimSing`, etc.).
    #[allow(clippy::too_many_arguments)]
    pub fn on_song_event(
        &mut self,
        event_kind: &str,
        value: &serde_json::Value,
        stage: Option<&mut Stage>,
        bf: Option<&mut BaseCharacter>,
        dad: Option<&mut BaseCharacter>,
        cam_flash_dur: &mut Option<f32>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        let StageScript::Sserafim {
            yunjin,
            chaewon,
            eunchae,
            visible,
            singing,
            lights_enabled,
            light_colors,
            light_durs,
            light_ints,
            light_count,
            light_idx,
            truck_light_alpha,
            truck_light_dur,
            cover_alpha,
            dark_amount,
            dark_start,
            dark_target,
            dark_elapsed,
            dark_dur,
            kick_waiting_door,
            ..
        } = &mut self.stage_script
        else {
            return out;
        };

        match event_kind {
            "sserafimShow" => {
                if let Some(arr) = value.get("visible").and_then(|v| v.as_array()) {
                    for (i, v) in arr.iter().take(5).enumerate() {
                        if let Some(b) = v.as_bool() {
                            visible[i] = b;
                        }
                    }
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.visible = visible[0];
                    }
                    if let Some(d) = dad {
                        d.visible = visible[1];
                    }
                    if let Some(c) = chaewon.as_deref_mut() {
                        c.visible = visible[2];
                    }
                    if let Some(e) = eunchae.as_deref_mut() {
                        e.visible = visible[3];
                    }
                    if let Some(b) = bf {
                        b.visible = visible[4];
                    }
                }
            }
            "sserafimSing" => {
                if let Some(arr) = value.get("singing").and_then(|v| v.as_array()) {
                    for (i, v) in arr.iter().take(6).enumerate() {
                        if let Some(b) = v.as_bool() {
                            singing[i] = b;
                        }
                    }
                }
            }
            "sserafimDark" => {
                *dark_start = *dark_amount;
                *dark_target = value
                    .get("amount")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                *dark_dur = value
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.1)
                    .max(0.0) as f32;
                *dark_elapsed = 0.0;
                if *dark_dur <= 0.0 {
                    *dark_amount = *dark_target;
                }
            }
            "sserafimLights" => {
                let amt = value
                    .get("amount")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.7) as f32;
                let dur = value
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5)
                    .max(0.05) as f32;
                *truck_light_alpha = amt.clamp(0.0, 1.0);
                *truck_light_dur = dur;
                if let Some(st) = stage {
                    for nm in ["truckLight1", "truckLight2"] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = *truck_light_alpha;
                        }
                    }
                }
            }
            "sserafimCover" => {
                let vis = value
                    .get("visible")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                *cover_alpha = if vis { 1.0 } else { 0.0 };
                if let Some(st) = stage {
                    if let Some(p) = st.get_named_prop_mut("solidCover") {
                        p.alpha = *cover_alpha;
                    }
                }
            }
            "sserafimFlash" => {
                let dur = value
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                *cam_flash_dur = Some(dur);
            }
            "sserafimPulseLights" => {
                *lights_enabled = value
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(cols) = value.get("colors").and_then(|v| v.as_array()) {
                    *light_count = cols.len().min(8);
                    for (i, c) in cols.iter().take(*light_count).enumerate() {
                        if let Some(s) = c.as_str() {
                            light_colors[i] = parse_stage_color(s);
                        }
                    }
                }
                if let Some(durs) = value.get("durations").and_then(|v| v.as_array()) {
                    for (i, d) in durs.iter().take(8).enumerate() {
                        if let Some(val) = d.as_f64() {
                            light_durs[i] = val.max(0.05) as f32;
                        }
                    }
                }
                if let Some(ints) = value.get("intensities").and_then(|v| v.as_array()) {
                    for (i, val) in ints.iter().take(8).enumerate() {
                        if let Some(n) = val.as_f64() {
                            light_ints[i] = n.clamp(0.0, 1.0) as f32;
                        }
                    }
                }
                *light_idx = 0;
            }
            "sserafimKick" => {
                let is_final = value
                    .get("final")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_final {
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.play_animation("kick2", true, false);
                        y.dance_every = 1.0;
                    }
                    *kick_waiting_door = true;
                    if let Some(slot) = self.sfx.door_kick[1] {
                        out.push(slot, 1.0);
                    }
                } else {
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.play_animation("kick1", true, false);
                    }
                    if let Some(slot) = self.sfx.door_kick[0] {
                        out.push(slot, 1.0);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Resets stage/character script timers cleanly on song retry.
    pub fn reset(&mut self, mut stage: Option<&mut Stage>) {
        self.spirit_trail.reset();
        self.speaker_shooter.reset();
        self.abot.reset();
        self.runners.reset();
        self.spraycans.reset();
        if let Some(st) = stage.as_deref_mut() {
            for prop in &mut st.props {
                prop.color = [1.0, 1.0, 1.0, 1.0];
                prop.wiggle = None;
            }
        }
        match &mut self.stage_script {
            StageScript::SpookyMansion {
                is_erect,
                lightning_strike_beat,
                lightning_strike_offset,
                flash_timer_s,
            } => {
                *lightning_strike_beat = 0;
                *lightning_strike_offset = 8;
                *flash_timer_s = None;
                if *is_erect {
                    if let Some(st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 0.0;
                            }
                        }
                    }
                }
            }
            StageScript::PhillyTrain {
                train_moving,
                train_started,
                train_finishing,
                train_cars,
                train_cooldown,
                train_timer_s,
                train_sound_ms,
                light_alpha,
                ..
            } => {
                *train_moving = false;
                *train_started = false;
                *train_finishing = false;
                *train_cars = 8;
                *train_cooldown = 0;
                *train_timer_s = 0.0;
                *train_sound_ms = 0.0;
                *light_alpha = 1.0;
                if let Some(st) = stage {
                    if let Some(train) = st.get_named_prop_mut("train") {
                        train.position[0] = 1480.0;
                    }
                }
            }
            StageScript::PhillyBlazin {
                sky_scroll_x,
                rain_time_scale,
                lightning_timer_s,
                lightning_fade_s,
                lightning_fading,
                ..
            } => {
                *sky_scroll_x = 0.0;
                *rain_time_scale = 1.0;
                *lightning_timer_s = 3.0;
                *lightning_fade_s = 0.0;
                *lightning_fading = false;
            }
            StageScript::LimoRide {
                fast_car_can_drive,
                fast_car_active,
                fast_car_timer_s,
                fast_car_vx,
            } => {
                *fast_car_can_drive = true;
                *fast_car_active = false;
                *fast_car_timer_s = 0.0;
                *fast_car_vx = 0.0;
                if let Some(st) = stage {
                    if let Some(car) = st.get_named_prop_mut("fastCar") {
                        car.position = [-12600.0, 360.0];
                    }
                }
            }
            StageScript::TankmanBattlefield { is_erect: true, .. } => {
                if let Some(st) = stage {
                    if let Some(bricks) = st.get_named_prop_mut("tankBricks") {
                        bricks.position = [445.0, 774.0];
                    }
                }
            }
            StageScript::PhillyStreets {
                lights_stop,
                last_change,
                change_interval,
                car_waiting,
                car_interruptable,
                car2_interruptable,
                car1,
                car2,
                ..
            } => {
                *lights_stop = false;
                *last_change = 0;
                *change_interval = 8;
                *car_waiting = false;
                *car_interruptable = true;
                *car2_interruptable = true;
                car1.active = false;
                car2.active = false;
                if let Some(st) = stage {
                    for nm in ["phillyCars", "phillyCars2"] {
                        if let Some(car) = st.get_named_prop_mut(nm) {
                            car.position = [1200.0, 818.0];
                            car.angle = 0.0;
                        }
                    }
                    if let Some(traffic) = st.get_named_prop_mut("phillyTraffic") {
                        if let Some(bop) = &mut traffic.bopper {
                            bop.play_animation("togreen", true);
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                visible,
                singing,
                lights_enabled,
                pulse_alpha,
                truck_light_alpha,
                cover_alpha,
                dark_amount,
                kick_waiting_door,
                ..
            } => {
                *visible = [true, false, false, false, false, false];
                *singing = [false; 6];
                *lights_enabled = false;
                *pulse_alpha = 0.0;
                *truck_light_alpha = 0.0;
                *cover_alpha = 0.0;
                *dark_amount = 0.0;
                *kick_waiting_door = false;
                if let Some(y) = yunjin.as_deref_mut() {
                    y.reset_character(false);
                    y.visible = true;
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.reset_character(false);
                    c.visible = false;
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.reset_character(false);
                    e.visible = false;
                }
                if let Some(st) = stage {
                    for nm in ["solidCover", "backLightColor", "backLightWhite", "truckLight1", "truckLight2", "truckDoor"] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = 0.0;
                        }
                    }
                }
            }
            StageScript::None | StageScript::School | StageScript::SchoolEvil { .. } | StageScript::TankmanBattlefield { .. } => {}
        }
    }

    /// Handles `FocusCamera` events for character scripts like A-Bot eye tracking.
    pub fn on_focus_character(&mut self, char_is_bf: bool, char_is_dad: bool) {
        if char_is_bf {
            self.abot.look_right();
        } else if char_is_dad {
            self.abot.look_left();
        }
    }

    /// Plays note-kind-aware singing/special animations for player & opponent (`parents-christmas`, `spooky-dark`, `sserafim`, `2hot`, etc.).
    pub fn on_note_hit(
        &mut self,
        is_player: bool,
        dir: NoteDirection,
        kind: NoteKindTag,
        mut character: Option<&mut BaseCharacter>,
        stage: Option<&Stage>,
    ) {
        if let StageScript::PhillyBlazin { rain_time_scale, .. } = &mut self.stage_script {
            *rain_time_scale += 0.7;
        }
        if let StageScript::Sserafim {
            yunjin,
            chaewon,
            eunchae,
            singing,
            ..
        } = &mut self.stage_script
        {
            if is_player && kind != NoteKindTag::NoAnim {
                if singing[0] {
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.play_sing_animation(dir, false, "");
                    }
                }
                if singing[2] {
                    if let Some(c) = chaewon.as_deref_mut() {
                        c.play_sing_animation(dir, false, "");
                    }
                }
                if singing[3] {
                    if let Some(e) = eunchae.as_deref_mut() {
                        e.play_sing_animation(dir, false, "");
                    }
                }
            }
        }
        if kind == NoteKindTag::NoAnim {
            return;
        }
        if is_player {
            match kind {
                NoteKindTag::Weekend1CockGun => {
                    self.abot.look_right();
                    self.spraycans.gun_cocked_timer_s = 1.0;
                    if let Some(c) = character {
                        c.play_animation("cock", true, true);
                    }
                    return;
                }
                NoteKindTag::Weekend1FireGun => {
                    self.abot.look_right();
                    self.spraycans.shoot();
                    if let Some(c) = character {
                        c.play_animation("shoot", true, true);
                    }
                    return;
                }
                _ => {}
            }
            if let Some(c) = character {
                let suffix = if kind == NoteKindTag::Alt { "alt" } else { "" };
                c.play_sing_animation(dir, false, suffix);
            }
        } else {
            match kind {
                NoteKindTag::Cheer => {
                    if let Some(c) = character.as_deref_mut() {
                        if c.id.eq_ignore_ascii_case("spooky-dark") {
                            c.hold_timer = 0.0;
                            c.play_animation("cheer", true, true);
                            return;
                        }
                    }
                }
                NoteKindTag::Ugh => {
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("ugh", true, true);
                        return;
                    }
                }
                NoteKindTag::HehPrettyGood => {
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("hehPrettyGood", true, true);
                        return;
                    }
                }
                NoteKindTag::Weekend1LightCan => {
                    self.abot.look_left();
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("lightCan", true, true);
                    }
                    return;
                }
                NoteKindTag::Weekend1KickCan => {
                    self.spraycans.spawn(stage);
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("kickCan", true, true);
                    }
                    return;
                }
                NoteKindTag::Weekend1KneeCan => {
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("kneeCan", true, true);
                    }
                    return;
                }
                _ => {}
            }
            if let Some(c) = character {
                let is_parents = c.id.eq_ignore_ascii_case("parents-christmas");
                let suffix = if (kind == NoteKindTag::Mom && is_parents)
                    || kind == NoteKindTag::Alt
                {
                    "alt"
                } else {
                    ""
                };
                c.play_sing_animation(dir, false, suffix);
            }
        }
    }

    /// Beat-triggered stage & character scripts (`spookyMansion`, `phillyTrain`, `limoRide`, `tankman`, `phillyStreets`, `sserafim`, `nene`/`abot`).
    pub fn on_beat_hit(
        &mut self,
        beat: i32,
        mut stage: Option<&mut Stage>,
        mut bf: Option<&mut BaseCharacter>,
        dad: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        self.abot.on_beat_hit(beat, gf.as_deref_mut(), &mut self.rng);

        match &mut self.stage_script {
            StageScript::SpookyMansion {
                is_erect,
                lightning_strike_beat,
                lightning_strike_offset,
                flash_timer_s,
            } => {
                let trigger_silent = beat == 4 && self.song_id.eq_ignore_ascii_case("spookeez");
                let trigger_rand = beat >= 0
                    && beat > (*lightning_strike_beat + *lightning_strike_offset)
                    && self.rng.bool_pct(10);
                if trigger_silent || trigger_rand {
                    *lightning_strike_beat = beat;
                    *lightning_strike_offset = self.rng.range_i32(8, 24);
                    if trigger_rand && !trigger_silent {
                        let s_idx = (self.rng.next_u32() & 1) as usize;
                        if let Some(slot) = self.sfx.thunder[s_idx] {
                            out.push(slot, 1.0);
                        }
                    }
                    if *is_erect {
                        *flash_timer_s = Some(0.0);
                        if let Some(st) = stage {
                            for nm in ["bgLight", "stairsLight"] {
                                if let Some(p) = st.get_named_prop_mut(nm) {
                                    p.alpha = 1.0;
                                }
                            }
                        }
                        if let Some(b) = bf.as_deref_mut() {
                            b.alpha = 0.0;
                        }
                        if let Some(d) = dad {
                            d.alpha = 0.0;
                        }
                        if let Some(g) = gf.as_deref_mut() {
                            g.alpha = 0.0;
                        }
                    } else if let Some(st) = stage {
                        if let Some(bg) = st.get_named_prop_mut("halloweenBG") {
                            if let Some(bop) = &mut bg.bopper {
                                bop.play_animation("lightning", true);
                            }
                        }
                    }
                    if let Some(b) = bf {
                        if b.has_animation("scared") && b.current_animation() != "cheer" {
                            b.play_animation("scared", true, true);
                        }
                    }
                    if let Some(g) = gf {
                        if g.has_animation("scared") {
                            g.play_animation("scared", true, true);
                        }
                    }
                }
            }
            StageScript::PhillyTrain {
                light_alpha,
                cur_light,
                train_moving,
                train_cooldown,
                ..
            } => {
                const LIGHT_COLORS: [[f32; 4]; 5] = [
                    [0.192, 0.635, 0.992, 1.0],
                    [0.192, 0.992, 0.549, 1.0],
                    [0.984, 0.200, 0.961, 1.0],
                    [0.984, 0.651, 0.200, 1.0],
                    [0.992, 0.271, 0.192, 1.0],
                ];
                if !*train_moving {
                    *train_cooldown += 1;
                }
                if beat >= 0
                    && beat % 8 == 4
                    && !*train_moving
                    && *train_cooldown > 8
                    && self.rng.bool_pct(30)
                {
                    *train_cooldown = self.rng.range_i32(-4, 0);
                    *train_moving = true;
                    if let Some(slot) = self.sfx.train_passes {
                        out.push(slot, 1.0);
                    }
                }
                if beat.rem_euclid(4) == 0 {
                    *cur_light = (self.rng.next_u32() as usize) % LIGHT_COLORS.len();
                    *light_alpha = 1.0;
                    if let Some(st) = stage {
                        if let Some(lights) = st.get_named_prop_mut("lights") {
                            lights.color = LIGHT_COLORS[*cur_light];
                            lights.alpha = 1.0;
                        }
                    }
                }
            }
            StageScript::LimoRide {
                fast_car_can_drive,
                fast_car_active,
                fast_car_timer_s,
                fast_car_vx,
            } => {
                if beat >= 0 && *fast_car_can_drive && self.rng.bool_pct(10) {
                    *fast_car_can_drive = false;
                    *fast_car_active = true;
                    *fast_car_timer_s = 0.0;
                    *fast_car_vx = self.rng.range_f32(170.0, 220.0) * 180.0;
                    let s_idx = (self.rng.next_u32() & 1) as usize;
                    if let Some(slot) = self.sfx.car_pass[s_idx] {
                        out.push(slot, 0.7);
                    }
                }
            }
            StageScript::TankmanBattlefield { is_erect: true, .. } => {
                if beat >= 0 && self.rng.bool_pct(2) {
                    if let Some(st) = stage {
                        if let Some(sniper) = st.get_named_prop_mut("sniper") {
                            if let Some(bop) = &mut sniper.bopper {
                                bop.play_animation("sip", false);
                            }
                        }
                    }
                }
            }
            StageScript::PhillyStreets {
                lights_stop,
                last_change,
                change_interval,
                car_waiting,
                car_interruptable,
                car2_interruptable,
                car1,
                car2,
                ..
            } => {
                if beat >= 0 && beat != (*last_change + *change_interval) && *car_interruptable && self.rng.bool_pct(10) {
                    *car_interruptable = false;
                    let variant = self.rng.range_i32(1, 4);
                    let anim_str = match variant {
                        1 => "car1",
                        2 => "car2",
                        3 => "car3",
                        _ => "car4",
                    };
                    if let Some(ref mut st) = stage {
                        if let Some(c) = st.get_named_prop_mut("phillyCars") {
                            if let Some(bop) = &mut c.bopper {
                                bop.play_animation(anim_str, true);
                            }
                        }
                    }
                    if !*lights_stop {
                        *car1 = QuadCarTween {
                            active: true,
                            elapsed_s: 0.0,
                            duration_s: match variant {
                                1 => self.rng.range_f32(1.0, 1.7),
                                2 => self.rng.range_f32(0.6, 1.2),
                                _ => self.rng.range_f32(1.5, 2.5),
                            },
                            start_delay_s: 0.0,
                            p0: [1263.4, 850.7],
                            p1: [2093.4, 761.7],
                            p2: [2795.4, 1058.7],
                            rot_start: -8.0,
                            rot_end: 18.0,
                            stopping_at_lights: false,
                        };
                    } else {
                        *car1 = QuadCarTween {
                            active: true,
                            elapsed_s: 0.0,
                            duration_s: match variant {
                                1 => self.rng.range_f32(1.0, 1.7),
                                2 => self.rng.range_f32(0.9, 1.5),
                                _ => self.rng.range_f32(1.5, 2.5),
                            },
                            start_delay_s: 0.0,
                            p0: [1173.4, 860.7],
                            p1: [1383.4, 835.7],
                            p2: [1563.4, 826.7],
                            rot_start: -7.0,
                            rot_end: -5.0,
                            stopping_at_lights: true,
                        };
                    }
                }
                if beat >= 0
                    && beat != (*last_change + *change_interval)
                    && *car2_interruptable
                    && !*lights_stop
                    && self.rng.bool_pct(10)
                {
                    *car2_interruptable = false;
                    let variant = self.rng.range_i32(1, 4);
                    let anim_str = match variant {
                        1 => "car1",
                        2 => "car2",
                        3 => "car3",
                        _ => "car4",
                    };
                    if let Some(ref mut st) = stage {
                        if let Some(c) = st.get_named_prop_mut("phillyCars2") {
                            if let Some(bop) = &mut c.bopper {
                                bop.play_animation(anim_str, true);
                            }
                        }
                    }
                    *car2 = QuadCarTween {
                        active: true,
                        elapsed_s: 0.0,
                        duration_s: match variant {
                            1 => self.rng.range_f32(1.0, 1.7),
                            2 => self.rng.range_f32(0.6, 1.2),
                            _ => self.rng.range_f32(1.5, 2.5),
                        },
                        start_delay_s: 0.0,
                        p0: [2795.4, 1018.7],
                        p1: [2093.4, 781.7],
                        p2: [1263.4, 870.7],
                        rot_start: 18.0,
                        rot_end: -8.0,
                        stopping_at_lights: false,
                    };
                }
                if beat >= 0 && beat == (*last_change + *change_interval) {
                    *last_change = beat;
                    *lights_stop = !*lights_stop;
                    if *lights_stop {
                        *change_interval = 20;
                        if let Some(ref mut st) = stage {
                            if let Some(t) = st.get_named_prop_mut("phillyTraffic") {
                                if let Some(bop) = &mut t.bopper {
                                    bop.play_animation("tored", true);
                                }
                            }
                        }
                    } else {
                        *change_interval = 30;
                        if let Some(ref mut st) = stage {
                            if let Some(t) = st.get_named_prop_mut("phillyTraffic") {
                                if let Some(bop) = &mut t.bopper {
                                    bop.play_animation("togreen", true);
                                }
                            }
                        }
                        if *car_waiting {
                            *car_waiting = false;
                            *car1 = QuadCarTween {
                                active: true,
                                elapsed_s: 0.0,
                                duration_s: self.rng.range_f32(1.8, 3.0),
                                start_delay_s: self.rng.range_f32(0.2, 1.2),
                                p0: [1563.4, 826.7],
                                p1: [2093.4, 761.7],
                                p2: [2795.4, 1058.7],
                                rot_start: -5.0,
                                rot_end: 18.0,
                                stopping_at_lights: false,
                            };
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                lights_enabled,
                light_colors,
                light_durs,
                light_ints,
                light_count,
                light_idx,
                pulse_alpha,
                pulse_dur,
                ..
            } => {
                if let Some(y) = yunjin.as_deref_mut() {
                    y.on_beat_hit(beat, false);
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.on_beat_hit(beat, false);
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.on_beat_hit(beat, false);
                }
                if *lights_enabled && *light_count > 0 {
                    let idx = *light_idx % *light_count;
                    *light_idx = (*light_idx + 1) % *light_count;
                    let col = light_colors[idx];
                    *pulse_dur = light_durs[idx].max(0.05);
                    *pulse_alpha = (light_ints[idx] * 0.8).clamp(0.0, 1.0);
                    if let Some(st) = stage {
                        if let Some(p) = st.get_named_prop_mut("backLightColor") {
                            p.color = col;
                            p.alpha = *pulse_alpha;
                        }
                        if let Some(p) = st.get_named_prop_mut("backLightWhite") {
                            p.alpha = (*pulse_alpha * 0.85).clamp(0.0, 1.0);
                        }
                    }
                }
            }
            StageScript::None
            | StageScript::School
            | StageScript::SchoolEvil { .. }
            | StageScript::PhillyBlazin { .. }
            | StageScript::TankmanBattlefield { .. } => {}
        }

        out
    }

    /// Per-frame update with zero heap allocations.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        elapsed_s: f32,
        song_pos_ms: f64,
        beat_length_ms: f64,
        health: f32,
        mut stage: Option<&mut Stage>,
        mut bf: Option<&mut BaseCharacter>,
        mut dad: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        self.spirit_trail.update(elapsed_s, dad.as_deref());
        let shoot_dir = self
            .speaker_shooter
            .update(song_pos_ms, gf.as_deref_mut(), &mut self.rng);
        if let Some(dir) = shoot_dir {
            self.abot.trigger_muzzle_flash(dir);
        }
        self.runners.update(elapsed_s, song_pos_ms, &mut self.rng);
        self.spraycans.update(elapsed_s, stage.as_deref_mut());

        if self.rain_enabled {
            let scale_mult = if let StageScript::PhillyBlazin { rain_time_scale, .. } = &self.stage_script {
                *rain_time_scale
            } else {
                1.0
            };
            self.rain_time += elapsed_s.max(0.0) * scale_mult;
            if song_pos_ms >= 0.0 && self.rain_duration_ms > 1.0 {
                let t = (song_pos_ms / self.rain_duration_ms).clamp(0.0, 1.0) as f32;
                self.rain_intensity = self.rain_start_intensity
                    + (self.rain_end_intensity - self.rain_start_intensity) * t;
            } else {
                self.rain_intensity = self.rain_start_intensity;
            }
        }

        let mut train_passing_flag = false;
        match &mut self.stage_script {
            StageScript::SpookyMansion {
                is_erect: true,
                flash_timer_s: Some(t),
                ..
            } => {
                *t += elapsed_s.max(0.0);
                if *t < 0.06 {
                    // Initial bright flash (silhouettes)
                } else if *t < 0.12 {
                    if let Some(ref mut st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 0.0;
                            }
                        }
                    }
                    if let Some(b) = bf.as_deref_mut() {
                        b.alpha = 1.0;
                    }
                    if let Some(d) = dad.as_deref_mut() {
                        d.alpha = 1.0;
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.alpha = 1.0;
                    }
                } else if *t < 1.62 {
                    let fade = ((*t - 0.12) / 1.5).clamp(0.0, 1.0);
                    if let Some(ref mut st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 1.0 - fade;
                            }
                        }
                    }
                    if let Some(b) = bf.as_deref_mut() {
                        b.alpha = fade;
                    }
                    if let Some(d) = dad.as_deref_mut() {
                        d.alpha = fade;
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.alpha = fade;
                    }
                } else {
                    if let Some(ref mut st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 0.0;
                            }
                        }
                    }
                    if let Some(b) = bf.as_deref_mut() {
                        b.alpha = 1.0;
                    }
                    if let Some(d) = dad.as_deref_mut() {
                        d.alpha = 1.0;
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.alpha = 1.0;
                    }
                    if let StageScript::SpookyMansion { flash_timer_s, .. } = &mut self.stage_script {
                        *flash_timer_s = None;
                    }
                }
            }
            StageScript::PhillyTrain {
                light_alpha,
                train_moving,
                train_started,
                train_finishing,
                train_cars,
                train_timer_s,
                train_sound_ms,
                ..
            } => {
                let beat_s = (beat_length_ms / 1000.0).max(0.1) as f32;
                *light_alpha = (*light_alpha - (beat_s * elapsed_s * 1.5)).max(0.0);
                if let Some(ref mut st) = stage {
                    if let Some(lights) = st.get_named_prop_mut("lights") {
                        lights.alpha = *light_alpha;
                    }
                }
                if *train_moving {
                    *train_sound_ms += elapsed_s * 1000.0;
                    *train_timer_s += elapsed_s;
                    if *train_timer_s >= 1.0 / 24.0 {
                        *train_timer_s = 0.0;
                        if *train_sound_ms >= 4700.0 {
                            *train_started = true;
                            if let Some(g) = gf.as_deref_mut() {
                                g.play_animation("hairBlow", false, false);
                            }
                        }
                        if *train_started {
                            if let Some(ref mut st) = stage {
                                if let Some(train) = st.get_named_prop_mut("train") {
                                    train.position[0] -= 400.0;
                                    if train.position[0] < -2000.0 && !*train_finishing {
                                        train.position[0] = -1150.0;
                                        *train_cars -= 1;
                                        if *train_cars <= 0 {
                                            *train_finishing = true;
                                        }
                                    }
                                    if train.position[0] < -4000.0 && *train_finishing {
                                        train.position[0] = 1480.0;
                                        *train_moving = false;
                                        *train_started = false;
                                        *train_finishing = false;
                                        *train_cars = 8;
                                        *train_sound_ms = 0.0;
                                        if let Some(g) = gf.as_deref_mut() {
                                            g.play_animation("hairFall", true, false);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                train_passing_flag = *train_started;
            }
            StageScript::PhillyBlazin {
                sky_scroll_x,
                rain_time_scale,
                lightning_timer_s,
                lightning_fade_s,
                lightning_fading,
                ..
            } => {
                let dt = elapsed_s.max(0.0);
                *sky_scroll_x -= dt * 35.0;
                let lerp_t = 1.0 - (-dt * 1.535 * 3.0).exp();
                *rain_time_scale += (0.02 - *rain_time_scale) * lerp_t;

                *lightning_timer_s -= dt;
                if *lightning_timer_s <= 0.0 {
                    *lightning_timer_s = self.rng.range_f32(7.0, 15.0);
                    *lightning_fading = true;
                    *lightning_fade_s = 0.0;
                    if let Some(ref mut st) = stage {
                        let lx = if self.rng.bool_pct(65) {
                            self.rng.range_i32(-250, 280) as f32
                        } else {
                            self.rng.range_i32(780, 900) as f32
                        };
                        if let Some(p) = st.get_named_prop_mut("lightning") {
                            p.visible = true;
                            p.position[0] = lx;
                            if let Some(bop) = &mut p.bopper {
                                bop.play_animation("strike", true);
                            }
                        }
                        if let Some(p) = st.get_named_prop_mut("skyAdditive") {
                            p.visible = true;
                            p.alpha = 0.7;
                        }
                        if let Some(p) = st.get_named_prop_mut("foregroundMultiply") {
                            p.visible = true;
                            p.alpha = 0.64;
                        }
                        if let Some(p) = st.get_named_prop_mut("additionalLighten") {
                            p.visible = true;
                            p.alpha = 0.3;
                        }
                    }
                    let s_idx = (self.rng.next_u32() as usize) % 3;
                    if let Some(slot) = self.sfx.lightning[s_idx] {
                        out.push(slot, 1.0);
                    }
                }

                if *lightning_fading {
                    *lightning_fade_s += dt;
                    let t_full = (*lightning_fade_s / 1.5).clamp(0.0, 1.0);
                    let t_fast = (*lightning_fade_s / 0.3).clamp(0.0, 1.0);
                    if let Some(ref mut st) = stage {
                        if let Some(p) = st.get_named_prop_mut("skyAdditive") {
                            p.alpha = 0.7 * (1.0 - t_full);
                            if t_full >= 1.0 {
                                p.visible = false;
                            }
                        }
                        if let Some(p) = st.get_named_prop_mut("foregroundMultiply") {
                            p.alpha = 0.64 * (1.0 - t_full);
                            if t_full >= 1.0 {
                                p.visible = false;
                            }
                        }
                        if let Some(p) = st.get_named_prop_mut("additionalLighten") {
                            p.alpha = 0.3 * (1.0 - t_fast);
                            if t_fast >= 1.0 {
                                p.visible = false;
                            }
                        }
                        if t_full >= 1.0 {
                            if let Some(p) = st.get_named_prop_mut("lightning") {
                                p.visible = false;
                            }
                        }
                    }
                    let char_c = 0.376 + (0.871 - 0.376) * t_fast;
                    let gf_c = 0.376 + (0.533 - 0.376) * t_fast;
                    if let Some(b) = bf {
                        b.color = [char_c, char_c, char_c, 1.0];
                    }
                    if let Some(d) = dad {
                        d.color = [char_c, char_c, char_c, 1.0];
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.color = [gf_c, gf_c, gf_c, 1.0];
                    }
                    if t_full >= 1.0 {
                        *lightning_fading = false;
                    }
                }
            }
            StageScript::SchoolEvil { time_s } => {
                *time_s += elapsed_s.max(0.0);
                let t = *time_s;
                if let Some(ref mut st) = stage {
                    for (nm, w) in [
                        ("backTrees", [t, 1.6, 1.6, 0.011]),
                        ("school", [t, 2.0, 4.0, 0.017]),
                        ("street", [t, 2.0, 4.0, 0.007]),
                        ("trees", [t, 2.0, 4.0, 0.007]),
                        ("evilstreet", [t, 2.0, 4.0, 0.007]),
                        ("backspikes", [t, 1.6, 1.6, 0.011]),
                        ("backspike", [t, 2.0, 4.0, 0.010]),
                    ] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.wiggle = Some(w);
                        }
                    }
                }
            }
            StageScript::LimoRide {
                fast_car_can_drive,
                fast_car_active,
                fast_car_timer_s,
                fast_car_vx,
            } => {
                if *fast_car_active {
                    *fast_car_timer_s += elapsed_s;
                    if let Some(ref mut st) = stage {
                        if let Some(car) = st.get_named_prop_mut("fastCar") {
                            car.position[0] += *fast_car_vx * elapsed_s;
                        }
                    }
                    if *fast_car_timer_s >= 2.0 {
                        *fast_car_active = false;
                        *fast_car_can_drive = true;
                        if let Some(ref mut st) = stage {
                            if let Some(car) = st.get_named_prop_mut("fastCar") {
                                car.position[0] = -12600.0;
                                car.position[1] = self.rng.range_i32(350, 380) as f32;
                            }
                        }
                    }
                }
            }
            StageScript::TankmanBattlefield {
                is_erect: false,
                clouds_x,
                tank_angle,
                tank_speed,
                ..
            } => {
                *clouds_x += 8.0 * elapsed_s;
                *tank_angle += elapsed_s * *tank_speed;
                if let Some(st) = stage {
                    if let Some(tank) = st.get_named_prop_mut("tankRolling") {
                        tank.angle = *tank_angle - 90.0 + 15.0;
                        let rad = (*tank_angle + 180.0).to_radians();
                        tank.position[0] = 400.0 + rad.cos() * 1500.0;
                        tank.position[1] = 1300.0 + rad.sin() * 1100.0;
                    }
                }
            }
            StageScript::PhillyStreets {
                sky_scroll_x,
                lights_stop,
                car_waiting,
                car_interruptable,
                car2_interruptable,
                car1,
                car2,
                ..
            } => {
                *sky_scroll_x -= elapsed_s * 22.0;
                if let Some(st) = stage {
                    if let Some(c1) = st.get_named_prop_mut("phillyCars") {
                        let was_stopping = car1.stopping_at_lights;
                        let [ref mut c1_x, ref mut c1_y] = c1.position;
                        let (_, done) = car1.eval(elapsed_s, c1_x, c1_y, &mut c1.angle);
                        if done {
                            if was_stopping {
                                if !*lights_stop {
                                    *car_waiting = false;
                                    *car1 = QuadCarTween {
                                        active: true,
                                        elapsed_s: 0.0,
                                        duration_s: self.rng.range_f32(1.8, 3.0),
                                        start_delay_s: self.rng.range_f32(0.2, 1.2),
                                        p0: [1563.4, 826.7],
                                        p1: [2093.4, 761.7],
                                        p2: [2795.4, 1058.7],
                                        rot_start: -5.0,
                                        rot_end: 18.0,
                                        stopping_at_lights: false,
                                    };
                                } else {
                                    *car_waiting = true;
                                }
                            } else {
                                *car_interruptable = true;
                            }
                        }
                    }
                    if let Some(c2) = st.get_named_prop_mut("phillyCars2") {
                        let [ref mut c2_x, ref mut c2_y] = c2.position;
                        let (_, done) = car2.eval(elapsed_s, c2_x, c2_y, &mut c2.angle);
                        if done {
                            *car2_interruptable = true;
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                dust_time_s,
                pulse_alpha,
                pulse_dur,
                truck_light_alpha,
                truck_light_dur,
                dark_amount,
                dark_start,
                dark_target,
                dark_elapsed,
                dark_dur,
                kick_waiting_door,
                ..
            } => {
                let dt = elapsed_s.max(0.0);
                *dust_time_s += dt;
                if let Some(y) = yunjin.as_deref_mut() {
                    y.update(dt, beat_length_ms, false);
                    if *kick_waiting_door && y.active_frame_snapshot().is_some_and(|(_, f, _)| f >= 23) {
                        *kick_waiting_door = false;
                        if let Some(ref mut st) = stage {
                            if let Some(d) = st.get_named_prop_mut("truckDoor") {
                                d.alpha = 1.0;
                            }
                        }
                    }
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.update(dt, beat_length_ms, false);
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.update(dt, beat_length_ms, false);
                }
                if *pulse_alpha > 0.0 {
                    *pulse_alpha = (*pulse_alpha - dt / (*pulse_dur).max(0.05)).max(0.0);
                    if let Some(ref mut st) = stage {
                        if let Some(p) = st.get_named_prop_mut("backLightColor") {
                            p.alpha = *pulse_alpha;
                        }
                        if let Some(p) = st.get_named_prop_mut("backLightWhite") {
                            p.alpha = (*pulse_alpha * 0.85).clamp(0.0, 1.0);
                        }
                    }
                }
                if *truck_light_alpha > 0.0 {
                    *truck_light_alpha =
                        (*truck_light_alpha - dt / (*truck_light_dur).max(0.05)).max(0.0);
                    if let Some(ref mut st) = stage {
                        for nm in ["truckLight1", "truckLight2"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = *truck_light_alpha;
                            }
                        }
                    }
                }
                if *dark_dur > 0.0 {
                    *dark_elapsed += dt;
                    let t = (*dark_elapsed / *dark_dur).clamp(0.0, 1.0);
                    *dark_amount = *dark_start + (*dark_target - *dark_start) * t;
                    if t >= 1.0 {
                        *dark_dur = 0.0;
                    }
                }
                let shade = (1.0 - *dark_amount).clamp(0.0, 1.0);
                if let Some(ref mut st) = stage {
                    for prop in &mut st.props {
                        if prop.name.as_deref() != Some("solidCover") {
                            prop.color = [shade, shade, shade, prop.color[3]];
                        }
                    }
                }
            }
            _ => {}
        }

        self.abot
            .update(elapsed_s, song_pos_ms, health, train_passing_flag, gf);
        out
    }

    /// Draws script-injected stage layers (`tankClouds`, `phillySkybox`, `phillyBlazin/skyBlur`,
    /// `TankmanSpriteGroup`, `SpraycanPool`, `Sserafim` members/dust, `SpiritTrail`, `ABotSpeaker`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_range(
        &self,
        batcher: &mut SpriteBatcher,
        z_start: i32,
        z_end: i32,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        dad: Option<&BaseCharacter>,
        gf: Option<&BaseCharacter>,
    ) {
        let in_range = |z: i32| z >= z_start && z < z_end;

        match &self.stage_script {
            StageScript::PhillyStreets {
                sky_tex: Some((tex, tex_sz)),
                sky_scroll_x,
                ..
            } if in_range(10) => {
                draw_tiled_x(
                    batcher,
                    *tex,
                    *tex_sz,
                    [-650.0, -375.0],
                    0.65,
                    [0.1, 0.1],
                    *sky_scroll_x,
                    cam_pos,
                    cam_zoom,
                );
            }
            StageScript::PhillyBlazin {
                sky_blur_tex: Some((tex, tex_sz)),
                sky_scroll_x,
                ..
            } if in_range(10) => {
                draw_tiled_x(
                    batcher,
                    *tex,
                    *tex_sz,
                    [-700.0, -120.0],
                    1.0,
                    [0.0, 0.0],
                    *sky_scroll_x,
                    cam_pos,
                    cam_zoom,
                );
            }
            StageScript::TankmanBattlefield {
                is_erect: false,
                clouds_tex: Some((tex, tex_sz)),
                clouds_x,
                ..
            } if in_range(12) => {
                draw_tiled_x(
                    batcher,
                    *tex,
                    *tex_sz,
                    [*clouds_x, 20.0],
                    1.0,
                    [0.25, 0.25],
                    0.0,
                    cam_pos,
                    cam_zoom,
                );
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                dust_mid_tex,
                dust_back_tex,
                dust_time_s,
                ..
            } => {
                if in_range(1000) {
                    if let Some(y) = yunjin.as_deref() {
                        y.draw(batcher, cam_pos, cam_zoom);
                    }
                    if let Some(c) = chaewon.as_deref() {
                        c.draw(batcher, cam_pos, cam_zoom);
                    }
                    if let Some(e) = eunchae.as_deref() {
                        e.draw(batcher, cam_pos, cam_zoom);
                    }
                }
                if in_range(2000) {
                    if let Some((tex, sz)) = dust_mid_tex {
                        draw_tiled_x(
                            batcher,
                            *tex,
                            *sz,
                            [-650.0, -200.0],
                            1.5,
                            [1.1, 1.1],
                            *dust_time_s * 350.0,
                            cam_pos,
                            cam_zoom,
                        );
                    }
                    if let Some((tex, sz)) = dust_back_tex {
                        draw_tiled_x(
                            batcher,
                            *tex,
                            *sz,
                            [-650.0, -250.0],
                            1.5,
                            [1.15, 1.15],
                            -*dust_time_s * 300.0,
                            cam_pos,
                            cam_zoom,
                        );
                    }
                }
            }
            _ => {}
        }

        if self.spraycans.enabled && in_range(290) {
            self.spraycans.draw(batcher, cam_pos, cam_zoom);
        }

        if self.runners.enabled && in_range(self.runners.z_index) {
            self.runners.draw(batcher, cam_pos, cam_zoom);
        }

        if self.spirit_trail.enabled && in_range(self.spirit_trail.z_index) {
            self.spirit_trail.draw(batcher, cam_pos, cam_zoom, dad);
        }

        if self.abot.enabled {
            let abot_z = self.abot.gf_z - 10;
            if in_range(abot_z) {
                if let Some(g) = gf {
                    self.abot.draw_back(batcher, g, cam_pos, cam_zoom);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tiled_x(
    batcher: &mut SpriteBatcher,
    tex: TextureHandle,
    tex_sz: [f32; 2],
    pos: [f32; 2],
    scale: f32,
    scroll: [f32; 2],
    scroll_x: f32,
    cam_pos: [f32; 2],
    cam_zoom: f32,
) {
    let tile_w = (tex_sz[0] * scale).max(1.0);
    let tile_h = tex_sz[1] * scale;
    let wrap = scroll_x.rem_euclid(tile_w);
    for copy in -1..=2 {
        let x = pos[0] + (copy as f32) * tile_w + wrap;
        let (sp, ss) = apply_camera_transform([x, pos[1]], [tile_w, tile_h], scroll, cam_pos, cam_zoom);
        batcher.push(
            tex,
            BlendMode::Normal,
            FilterMode::Linear,
            SpriteInstance::new(sp, ss),
        );
    }
}

fn apply_erect_adjust_color(
    stage_lower: &str,
    stage: Option<&mut Stage>,
    bf: Option<&mut BaseCharacter>,
    dad: Option<&mut BaseCharacter>,
    gf: Option<&mut BaseCharacter>,
) {
    let set_char = |ch: Option<&mut BaseCharacter>, hsbc: [f32; 4]| {
        if let Some(c) = ch {
            c.adjust_color = Some(hsbc);
        }
    };
    let set_prop = |st: &mut Stage, name: &str, hsbc: [f32; 4]| {
        if let Some(p) = st.get_named_prop_mut(name) {
            p.adjust_color = Some(hsbc);
        }
    };

    match stage_lower {
        "mainstageerect" => {
            set_char(bf, [12.0, 0.0, -23.0, 7.0]);
            set_char(gf, [-9.0, 0.0, -30.0, -4.0]);
            set_char(dad, [-32.0, 0.0, -33.0, -23.0]);
        }
        "limorideerect" => {
            let hsbc = [-30.0, -20.0, -30.0, 0.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
            if let Some(st) = stage {
                for name in [
                    "limoDancer1",
                    "limoDancer2",
                    "limoDancer3",
                    "limoDancer4",
                    "limoDancer5",
                    "bgLimo",
                ] {
                    set_prop(st, name, hsbc);
                }
            }
        }
        "mallxmaserect" => {
            let chars_hsbc = [5.0, 20.0, 0.0, 0.0];
            set_char(bf, chars_hsbc);
            set_char(gf, chars_hsbc);
            set_char(dad, chars_hsbc);
            if let Some(st) = stage {
                let boppers_hsbc = [15.0, 0.0, 20.0, 0.0];
                set_prop(st, "bottomBoppers", boppers_hsbc);
                set_prop(st, "upperBoppers", boppers_hsbc);
            }
        }
        "phillytrainerect" => {
            let hsbc = [-26.0, -16.0, -5.0, 0.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
        }
        "phillystreetserect" => {
            let hsbc = [-5.0, -40.0, -20.0, -25.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
            if let Some(st) = stage {
                for name in ["phillyCars", "phillyCars2", "phillyTraffic", "spraycanPile"] {
                    set_prop(st, name, hsbc);
                }
            }
        }
        "schoolerect" => {
            set_char(bf, [-10.0, 0.0, -16.0, 12.0]);
            set_char(gf, [-10.0, 0.0, -20.0, 8.0]);
            set_char(dad, [-10.0, 0.0, -16.0, 12.0]);
        }
        "tankmanbattlefielderect" => {
            let hsbc = [-38.0, -20.0, -46.0, -25.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn abot_analyzer_fft_responds_to_sine_and_respects_mute_and_preroll() {
        let sr = 44100u32;
        let frames = 44100usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (2.0 * PI * 440.0 * (i as f32) / (sr as f32)).sin() * 0.6;
            samples.push(s);
            samples.push(s);
        }
        let audio = DecodedAudio {
            samples: Arc::from(samples),
            frames,
            sample_rate: sr,
        };
        let mut analyzer = ABotSpectralAnalyzer::new();
        analyzer.inst_audio = Some((audio, 0));

        let mut levels = [0u8; 7];
        // Pre-roll countdown (`song_pos_ms < 0`): all bars silent.
        analyzer.analyze(-500.0, 1.0 / 60.0, &mut levels);
        assert_eq!(levels, [0; 7]);

        // Mid-song at 440Hz tone: band 3 (~270..811Hz) activates.
        analyzer.analyze(500.0, 1.0 / 60.0, &mut levels);
        assert!(levels[3] >= 3, "expected 440Hz bar activation, got {levels:?}");

        // Muted: immediately drops to 0.
        analyzer.audible = false;
        analyzer.analyze(500.0, 1.0 / 60.0, &mut levels);
        assert_eq!(levels, [0; 7]);
    }
}
