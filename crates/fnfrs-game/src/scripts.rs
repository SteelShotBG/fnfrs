//! Native Rust implementations of base Friday Night Funkin' V-Slice `.hxc` scripts
//! (characters, note kinds, and stage dynamics) with zero per-frame heap allocations.

use std::f32::consts::PI;

use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BlendMode, EvaluatedPart, FilterMode, GpuContext, SparrowAtlas,
    SpriteBatcher, SpriteInstance, TextureHandle, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_ROTATED_90_CCW,
};

use crate::character::{
    apply_camera_transform, draw_animate_parts, upload_png_path, AtlasDrawParams, BaseCharacter,
};
use crate::input::NoteDirection;
use crate::stage::Stage;

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
    fn push(&mut self, slot: usize, gain: f32) {
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

    fn update(&mut self, song_pos_ms: f64, gf: Option<&mut BaseCharacter>, rng: &mut ScriptRng) {
        if !self.enabled {
            return;
        }
        let Some(gf) = gf else {
            return;
        };
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
        }
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
    fn draw(&self, batcher: &mut SpriteBatcher, pos: [f32; 2], scale: f32, scroll: [f32; 2], cam_pos: [f32; 2], cam_zoom: f32, color: [f32; 4]) {
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
            ref_unscaled_size: None,
        };
        draw_animate_parts(
            batcher,
            &self.textures,
            &self.parts_buf,
            self.bounds_origin,
            self.bounds_size,
            &params,
            false,
            false,
        );
    }
}

#[derive(Debug)]
struct ABotSpeaker {
    enabled: bool,
    is_dark: bool,
    gf_z: i32,
    abot: Option<LoadedAnimateProp>,
    eyes: Option<LoadedAnimateProp>,
    stereo_bg: Option<(TextureHandle, [f32; 2])>,
    white_tex: TextureHandle,
    viz_tex: Option<(TextureHandle, [f32; 2])>,
    viz_bars: [Vec<AtlasFrame>; 7],
    viz_levels: [u8; 7],
    viz_decay_s: f32,
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
            gf_z: 300,
            abot: None,
            eyes: None,
            stereo_bg: None,
            white_tex: TextureHandle(0),
            viz_tex: None,
            viz_bars: std::array::from_fn(|_| Vec::new()),
            viz_levels: [0; 7],
            viz_decay_s: 0.0,
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

        let gf_z = gf.map_or(300, |g| g.z_index);
        let nene_knife_enabled = is_nene;
        Self {
            enabled: abot.is_some() || stereo_bg.is_some(),
            is_dark,
            gf_z,
            abot,
            eyes,
            stereo_bg,
            white_tex: gpu.white_texture(),
            viz_tex,
            viz_bars,
            viz_levels: [0; 7],
            viz_decay_s: 0.0,
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
        if let Some(a) = &mut self.abot {
            a.play_from(1);
        }
        if let Some(e) = &mut self.eyes {
            e.play_from(17);
        }
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

    fn pulse_viz(&mut self, is_beat: bool, rng: &mut ScriptRng) {
        for (idx, level) in self.viz_levels.iter_mut().enumerate() {
            let base = if is_beat { 4 } else { 2 };
            let jitter = rng.range_i32(0, 2);
            let band_bias = match idx {
                0 | 1 | 6 => 1,
                _ => 0,
            };
            *level = (*level).max((base + jitter + band_bias).clamp(1, 6) as u8);
        }
    }

    fn on_beat_hit(&mut self, beat: i32, gf: Option<&mut BaseCharacter>, rng: &mut ScriptRng) {
        if !self.enabled {
            return;
        }
        if let Some(a) = &mut self.abot {
            a.play_from(1);
        }
        if beat >= 0 {
            self.pulse_viz(true, rng);
        }
        if self.nene_knife_enabled {
            if let Some(gf) = gf {
                match self.nene_state {
                    NeneState::PreRaise => {
                        gf.play_animation("danceLeft", false, false);
                        gf.has_danced = false;
                    }
                    NeneState::Ready => {
                        if self.blink_countdown <= 0 {
                            gf.play_animation("idleKnife", false, false);
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

        self.viz_decay_s += elapsed_s.max(0.0);
        if self.viz_decay_s >= 1.0 / 24.0 {
            self.viz_decay_s = 0.0;
            for lv in &mut self.viz_levels {
                if *lv > 0 {
                    *lv -= 1;
                }
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

    fn base_pos(gf: &BaseCharacter) -> [f32; 2] {
        // nene.hxc: abot.x = this.x - 95 - (-globalOffsets[0] * scale.x), abot.y = this.y + 384 - (-globalOffsets[1] * scale.y)
        [
            gf.position[0] - 95.0 + gf.global_offsets[0] * gf.scale,
            gf.position[1] + 384.0 + gf.global_offsets[1] * gf.scale,
        ]
    }

    fn draw_back(&self, batcher: &mut SpriteBatcher, gf: &BaseCharacter, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.enabled || gf.alpha <= 0.0 {
            return;
        }
        let ab = Self::base_pos(gf);
        let scroll = gf.scroll_factor;
        let tint = if self.is_dark {
            [0.68, 0.72, 0.85, gf.alpha]
        } else {
            [1.0, 1.0, 1.0, gf.alpha]
        };

        // eyeWhites (`z = gf_z - 20`)
        let (ew_pos, ew_sz) = apply_camera_transform([ab[0] + 40.0, ab[1] + 250.0], [160.0, 60.0], scroll, cam_pos, cam_zoom);
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Linear,
            SpriteInstance::new(ew_pos, ew_sz).with_color([tint[0], tint[1], tint[2], gf.alpha]),
        );

        // stereoBG (`z = gf_z - 18`)
        if let Some((tex, sz)) = self.stereo_bg {
            let (p, s) = apply_camera_transform([ab[0] + 150.0, ab[1] + 30.0], sz, scroll, cam_pos, cam_zoom);
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new(p, s).with_color(tint),
            );
        }

        // eyes (`z = gf_z - 15`)
        if let Some(e) = &self.eyes {
            e.draw(batcher, [ab[0] + 50.0, ab[1] + 238.0], gf.scale, scroll, cam_pos, cam_zoom, tint);
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
                batcher.push(
                    viz_tex,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new(p, s)
                        .with_uv_rect(frame.uv_bounds_inset(viz_tex_sz[0], viz_tex_sz[1], 0.5))
                        .with_color(tint)
                        .with_flags(flags),
                );
            }
        }

        // abot speaker shell (`z = gf_z - 10`)
        if let Some(a) = &self.abot {
            a.draw(batcher, ab, gf.scale, scroll, cam_pos, cam_zoom, [1.0, 1.0, 1.0, gf.alpha]);
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
            scale: 0.4,
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
    z_index: i32,
    texture: Option<TextureHandle>,
    tex_size: [f32; 2],
    run_frames: Vec<AtlasFrame>,
    shot_frames: [Vec<AtlasFrame>; 2],
    timemap: Vec<(f64, bool)>,
    next_spawn: usize,
    pool: [TankRunner; 8],
}

impl Default for TankmanRunners {
    fn default() -> Self {
        Self {
            enabled: false,
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
        stage_id: &str,
        picospeaker_notes: &[SongNoteData],
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        rng: &mut ScriptRng,
    ) -> Self {
        let stage_lower = stage_id.to_ascii_lowercase();
        if (stage_lower != "tankmanbattlefield" && stage_lower != "tankmanbattlefielderect")
            || picospeaker_notes.is_empty()
        {
            return Self::default();
        }
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
                    scale: 0.4,
                    x: 9999.0,
                    y: (200 + rng.range_i32(50, 100)) as f32,
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
                    let flick = (r.flicker_timer_s * 20.0) as i32;
                    r.visible = (flick & 1) == 0;
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
            let trimmed = frame.upright_trimmed_size();
            let draw_x = if r.shot { r.x - 300.0 } else { r.x } + (-frame.frame_x) * r.scale;
            let draw_y = if r.shot { r.y - 200.0 } else { r.y } + (-frame.frame_y) * r.scale;
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
            if !r.going_right {
                flags |= SPRITE_FLAG_FLIP_X;
            }
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new(pos, size)
                    .with_uv_rect(frame.uv_bounds_inset(self.tex_size[0], self.tex_size[1], 1.0))
                    .with_flags(flags),
            );
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

#[derive(Debug)]
enum StageScript {
    None,
    SpookyMansion {
        #[allow(dead_code)]
        is_erect: bool,
        lightning_strike_beat: i32,
        lightning_strike_offset: i32,
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
    LimoRide {
        fast_car_can_drive: bool,
        fast_car_active: bool,
        fast_car_timer_s: f32,
        fast_car_vx: f32,
    },
    School,
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
}

/// Coordinates all hardcoded V-Slice stage and character `.hxc` behaviors cleanly
/// with zero heap allocations inside update/render loops.
#[derive(Debug)]
pub struct NativeScriptHost {
    song_id: String,
    stage_script: StageScript,
    spirit_trail: SpiritTrail,
    speaker_shooter: SpeakerShooter,
    abot: ABotSpeaker,
    runners: TankmanRunners,
    sfx: ScriptSfxSlots,
    rng: ScriptRng,
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
            }
        } else if stage_lower == "phillytrain" {
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
        } else if stage_lower == "limoride" {
            StageScript::LimoRide {
                fast_car_can_drive: true,
                fast_car_active: false,
                fast_car_timer_s: 0.0,
                fast_car_vx: 0.0,
            }
        } else if stage_lower == "school" || stage_lower == "schoolerect" {
            StageScript::School
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
        } else {
            StageScript::None
        };

        let spirit_trail = SpiritTrail {
            enabled: dad_id.eq_ignore_ascii_case("spirit"),
            ..SpiritTrail::default()
        };
        let speaker_shooter = SpeakerShooter::new(gf_id, picospeaker_notes);
        let abot = ABotSpeaker::load(gf_id, stage_id, gf, assets, gpu);
        let runners = TankmanRunners::load(stage_id, picospeaker_notes, assets, gpu, &mut rng);

        Self {
            song_id: song_id.to_owned(),
            stage_script,
            spirit_trail,
            speaker_shooter,
            abot,
            runners,
            sfx,
            rng,
        }
    }

    /// Minimal empty host for tests & headless scenes without a loaded stage script.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            song_id: String::new(),
            stage_script: StageScript::None,
            spirit_trail: SpiritTrail::default(),
            speaker_shooter: SpeakerShooter::default(),
            abot: ABotSpeaker::default(),
            runners: TankmanRunners::default(),
            sfx: ScriptSfxSlots::default(),
            rng: ScriptRng(0x5A17_C0DE),
        }
    }

    /// Applies one-time initial stage setup after characters & stage props are placed.
    pub fn on_stage_ready(&mut self, stage: Option<&mut Stage>) {
        if let StageScript::School = self.stage_script {
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
        } else if let StageScript::TankmanBattlefield { is_erect: true, .. } = self.stage_script {
            if let Some(st) = stage {
                if let Some(bricks) = st.get_named_prop_mut("tankBricks") {
                    bricks.position = [445.0, 774.0];
                }
            }
        } else if let StageScript::PhillyTrain { .. } = self.stage_script {
            if let Some(st) = stage {
                if let Some(train) = st.get_named_prop_mut("train") {
                    train.position[0] = 1480.0;
                }
            }
        } else if let StageScript::LimoRide { .. } = self.stage_script {
            if let Some(st) = stage {
                if let Some(car) = st.get_named_prop_mut("fastCar") {
                    car.position = [-12600.0, 360.0];
                }
            }
        } else if let StageScript::PhillyStreets { .. } = self.stage_script {
            if let Some(st) = stage {
                if let Some(traffic) = st.get_named_prop_mut("phillyTraffic") {
                    if let Some(bop) = &mut traffic.bopper {
                        bop.play_animation("togreen", true);
                    }
                }
            }
        }
    }

    /// Resets stage/character script timers cleanly on song retry.
    pub fn reset(&mut self, stage: Option<&mut Stage>) {
        self.spirit_trail.reset();
        self.speaker_shooter.reset();
        self.abot.reset();
        self.runners.reset();
        match &mut self.stage_script {
            StageScript::SpookyMansion {
                lightning_strike_beat,
                lightning_strike_offset,
                ..
            } => {
                *lightning_strike_beat = 0;
                *lightning_strike_offset = 8;
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
            _ => {}
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

    /// Plays note-kind-aware singing/special animations for player & opponent (`parents-christmas`, `spooky-dark`, etc.).
    pub fn on_note_hit(
        &mut self,
        is_player: bool,
        dir: NoteDirection,
        kind: NoteKindTag,
        mut character: Option<&mut BaseCharacter>,
    ) {
        if kind == NoteKindTag::NoAnim {
            return;
        }
        if is_player {
            match kind {
                NoteKindTag::Weekend1CockGun => {
                    self.abot.look_right();
                    if let Some(c) = character {
                        c.play_animation("cock", true, true);
                    }
                    return;
                }
                NoteKindTag::Weekend1FireGun => {
                    self.abot.look_right();
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

    /// Beat-triggered stage & character scripts (`spookyMansion`, `phillyTrain`, `limoRide`, `tankman`, `phillyStreets`, `nene`/`abot`).
    pub fn on_beat_hit(
        &mut self,
        beat: i32,
        mut stage: Option<&mut Stage>,
        bf: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        self.abot.on_beat_hit(beat, gf.as_deref_mut(), &mut self.rng);

        match &mut self.stage_script {
            StageScript::SpookyMansion {
                lightning_strike_beat,
                lightning_strike_offset,
                ..
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
                    if let Some(st) = stage {
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
            StageScript::None | StageScript::School | StageScript::TankmanBattlefield { .. } => {}
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
        dad: Option<&BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) {
        self.spirit_trail.update(elapsed_s, dad);
        self.speaker_shooter
            .update(song_pos_ms, gf.as_deref_mut(), &mut self.rng);
        self.runners.update(elapsed_s, song_pos_ms, &mut self.rng);

        let mut train_passing_flag = false;
        match &mut self.stage_script {
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
            _ => {}
        }

        self.abot.update(elapsed_s, health, train_passing_flag, gf);
    }

    /// Draws script-injected stage layers (`tankClouds`, `phillySkybox`, `TankmanSpriteGroup`,
    /// `SpiritTrail`, `ABotSpeaker`) cleanly interleaved in `[z_min, z_max)` ascending order.
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
            _ => {}
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
