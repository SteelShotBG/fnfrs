use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BlendMode, EvaluatedPart, FilterMode, GpuContext, SparrowAtlas,
    SpriteBatcher, SpriteInstance, TextureHandle, SPRITE_FLAG_ROTATED_90_CCW,
};

use super::abot_vis::ABotSpectralAnalyzer;
use super::ScriptRng;
use crate::character::{
    apply_camera_transform, draw_animate_parts, upload_png_path, AtlasDrawParams, BaseCharacter,
};
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
    pub(super) fn load(assets: &AssetLocator, gpu: &mut GpuContext, rel_dir: &str) -> Option<Self> {
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
#[derive(Debug)]
pub(super) struct ABotSpeaker {
    pub(super) enabled: bool,
    is_dark: bool,
    is_otis: bool,
    is_tankmen: bool,
    pub(super) gf_z: i32,
    abot: Option<LoadedAnimateProp>,
    eyes: Option<LoadedAnimateProp>,
    stereo_bg: Option<(TextureHandle, [f32; 2])>,
    white_tex: TextureHandle,
    viz_tex: Option<(TextureHandle, [f32; 2])>,
    viz_bars: [Vec<AtlasFrame>; 7],
    viz_levels: [u8; 7],
    pub(super) analyzer: ABotSpectralAnalyzer,
    muzzle_tex: Option<(TextureHandle, [f32; 2])>,
    muzzle_anims: [Vec<AtlasFrame>; 4],
    muzzle_active: bool,
    muzzle_dir: usize,
    muzzle_offset: [f32; 2],
    muzzle_time_s: f32,
    pupil_state: PupilLook,
    pub(super) nene_knife_enabled: bool,
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
    pub(super) fn load(
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

    pub(super) fn reset(&mut self) {
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

    pub(super) fn trigger_muzzle_flash(&mut self, dir: usize) {
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

    pub(super) fn look_left(&mut self) {
        if self.pupil_state != PupilLook::Left {
            self.pupil_state = PupilLook::Left;
            if let Some(e) = &mut self.eyes {
                e.play_from(0);
            }
        }
    }

    pub(super) fn look_right(&mut self) {
        if self.pupil_state != PupilLook::Right {
            self.pupil_state = PupilLook::Right;
            if let Some(e) = &mut self.eyes {
                e.play_from(17);
            }
        }
    }

    pub(super) fn on_beat_hit(&mut self, beat: i32, gf: Option<&mut BaseCharacter>, rng: &mut ScriptRng) {
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

    pub(super) fn update(
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

    pub(super) fn draw_back(&self, batcher: &mut SpriteBatcher, gf: &BaseCharacter, cam_pos: [f32; 2], cam_zoom: f32) {
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
