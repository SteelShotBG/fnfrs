//! V-Slice Title Screen (`TitleState.hx`).

use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BitmapFont, BlendMode, EvaluatedPart, FilterMode, GpuContext,
    SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle,
    SPRITE_FLAG_AFFINE, SPRITE_FLAG_ROTATED_90_CCW,
};

use super::{draw_menu_sparrow_frame, load_menu_image, load_menu_sparrow, MenuCommand, MenuInputState};

const INTRO_BPM: f32 = 102.0;

#[derive(Debug)]
struct WackyPair {
    line1: String,
    line2: String,
}

#[derive(Debug)]
pub struct TitleScreenState {
    white_tex: TextureHandle,
    logo_tex: Option<(TextureHandle, [f32; 2])>,
    logo_frames: Vec<AtlasFrame>,
    logo_frame: usize,
    gf_tex: Option<(TextureHandle, [f32; 2])>,
    gf_left_frames: Vec<AtlasFrame>,
    gf_right_frames: Vec<AtlasFrame>,
    gf_dance_left: bool,
    gf_anim_time_s: f32,

    title_text_atlas: Option<AnimateAtlas>,
    title_text_textures: Vec<(TextureHandle, [f32; 2])>,
    title_text_parts: Vec<EvaluatedPart>,
    title_text_bounds: ([f32; 2], [f32; 2]),
    title_text_pressed: bool,
    title_text_time_s: f32,

    ng_tex: Option<(TextureHandle, [f32; 2])>,
    ng_visible: bool,

    intro_lines: [StackTextBuffer<64>; 4],
    intro_line_count: usize,
    wacky: WackyPair,

    elapsed_s: f32,
    beat_timer_s: f32,
    cur_beat: i32,
    skipped_intro: bool,
    transitioning: bool,
    transition_timer_s: f32,
    flash_alpha: f32,
    hue_shift: f32,
    rainbow_mode: bool,
    cheat_pos: usize,
}

impl TitleScreenState {
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext, skip_intro: bool) -> Self {
        let logo_loaded = load_menu_sparrow(assets, gpu, "logoBumpin");
        let (logo_tex, logo_frames) = match logo_loaded {
            Some((tex, sz, atlas)) => (
                Some((tex, sz)),
                atlas.frames_by_prefix("logo bumpin").cloned().collect(),
            ),
            None => (None, Vec::new()),
        };

        let gf_loaded = load_menu_sparrow(assets, gpu, "gfDanceTitle");
        let (gf_tex, gf_left_frames, gf_right_frames) = match gf_loaded {
            Some((tex, sz, atlas)) => {
                let all: Vec<AtlasFrame> = atlas.frames_by_prefix("gfDance").cloned().collect();
                let mut left = Vec::with_capacity(16);
                let mut right = Vec::with_capacity(16);
                for &idx in &[30usize, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14] {
                    if let Some(f) = all.get(idx) {
                        left.push(f.clone());
                    }
                }
                for idx in 15..=29usize {
                    if let Some(f) = all.get(idx) {
                        right.push(f.clone());
                    }
                }
                (Some((tex, sz)), left, right)
            }
            None => (None, Vec::new(), Vec::new()),
        };

        let title_text = load_title_text_atlas(assets, gpu);
        let title_text_atlas = title_text.atlas;
        let title_text_textures = title_text.textures;
        let title_text_bounds = title_text.bounds;
        let ng_tex = load_menu_image(assets, gpu, "newgrounds_logo");
        let wacky = load_random_wacky(assets);

        Self {
            white_tex: gpu.white_texture(),
            logo_tex,
            logo_frames,
            logo_frame: 0,
            gf_tex,
            gf_left_frames,
            gf_right_frames,
            gf_dance_left: false,
            gf_anim_time_s: 0.0,
            title_text_atlas,
            title_text_textures,
            title_text_parts: Vec::with_capacity(32),
            title_text_bounds,
            title_text_pressed: false,
            title_text_time_s: 0.0,
            ng_tex,
            ng_visible: false,
            intro_lines: std::array::from_fn(|_| StackTextBuffer::new()),
            intro_line_count: 0,
            wacky,
            elapsed_s: 0.0,
            beat_timer_s: 0.0,
            cur_beat: 0,
            skipped_intro: skip_intro,
            transitioning: false,
            transition_timer_s: 0.0,
            flash_alpha: if skip_intro { 0.85 } else { 0.0 },
            hue_shift: 0.0,
            rainbow_mode: false,
            cheat_pos: 0,
        }
    }

    pub fn reset(&mut self, skip_intro: bool) {
        self.transitioning = false;
        self.transition_timer_s = 0.0;
        self.title_text_pressed = false;
        self.title_text_time_s = 0.0;
        self.skipped_intro = skip_intro;
        self.flash_alpha = if skip_intro { 0.8 } else { 0.0 };
        self.ng_visible = false;
        self.intro_line_count = 0;
        self.cur_beat = 0;
        self.beat_timer_s = 0.0;
        self.cheat_pos = 0;
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        let dt = elapsed_s.clamp(0.0, 0.1);
        self.elapsed_s += dt;
        self.gf_anim_time_s += dt;
        self.title_text_time_s += dt;

        if self.flash_alpha > 0.0 {
            self.flash_alpha = (self.flash_alpha - dt * 1.4).max(0.0);
        }

        if input.left_held {
            self.hue_shift = (self.hue_shift - dt * 0.1).rem_euclid(1.0);
        }
        if input.right_held {
            self.hue_shift = (self.hue_shift + dt * 0.1).rem_euclid(1.0);
        }

        // Advance 24 FPS logo & title-screen-text Animate playback.
        if !self.logo_frames.is_empty() {
            let f = (self.beat_timer_s * 24.0).floor() as usize;
            self.logo_frame = f.min(self.logo_frames.len() - 1);
        }
        if let Some(atlas) = &self.title_text_atlas {
            let label = if self.title_text_pressed {
                "Confirm"
            } else {
                "Idle"
            };
            let (start, len) = atlas
                .label_span(label)
                .map(|s| (s.start, s.duration.max(1)))
                .unwrap_or((0, atlas.total_frames().max(1)));
            let local = ((self.title_text_time_s * 24.0).floor() as u32) % len;
            atlas.evaluate_frame(start + local, &mut self.title_text_parts);
        }

        // 102 BPM beat tracker (`freakyMenu-metadata.json`).
        let beat_dur = 60.0 / INTRO_BPM;
        self.beat_timer_s += dt;
        while self.beat_timer_s >= beat_dur {
            self.beat_timer_s -= beat_dur;
            self.cur_beat += 1;
            self.on_beat_hit(self.cur_beat);
        }

        if self.skipped_intro && !self.rainbow_mode {
            self.update_cheat_code(input);
        }

        if input.back_pressed {
            return MenuCommand::PlaySfx("cancelMenu");
        }

        if input.accept_pressed {
            if !self.skipped_intro {
                self.skip_intro();
                return MenuCommand::None;
            }
            if self.transitioning {
                return MenuCommand::EnterMainMenu;
            }
            self.transitioning = true;
            self.transition_timer_s = 0.0;
            self.title_text_pressed = true;
            self.title_text_time_s = 0.0;
            self.flash_alpha = 1.0;
            return MenuCommand::PlaySfx("confirmMenu");
        }

        if self.transitioning {
            self.transition_timer_s += dt;
            if self.transition_timer_s >= 1.5 {
                self.transitioning = false;
                return MenuCommand::EnterMainMenu;
            }
        }

        MenuCommand::None
    }

    fn update_cheat_code(&mut self, input: MenuInputState) {
        // Konami-style arrow sequence from TitleState.hx: D, U, D, U, L, R, L, R.
        const SEQ: [u8; 8] = [1, 2, 1, 2, 0, 3, 0, 3];
        let pressed = if input.left_pressed {
            Some(0u8)
        } else if input.down_pressed {
            Some(1u8)
        } else if input.up_pressed {
            Some(2u8)
        } else if input.right_pressed {
            Some(3u8)
        } else {
            None
        };
        if let Some(dir) = pressed {
            if dir == SEQ[self.cheat_pos] {
                self.cheat_pos += 1;
                if self.cheat_pos >= SEQ.len() {
                    self.rainbow_mode = true;
                    self.flash_alpha = 1.0;
                }
            } else {
                self.cheat_pos = 0;
            }
        }
    }

    fn on_beat_hit(&mut self, beat: i32) {
        if !self.skipped_intro {
            match beat {
                1 => self.set_intro_lines(&["The", "Funkin Crew Inc"]),
                3 => self.push_intro_line("presents"),
                4 => self.clear_intro_lines(),
                5 => self.set_intro_lines(&["In association", "with"]),
                7 => {
                    self.push_intro_line("newgrounds");
                    self.ng_visible = true;
                }
                8 => {
                    self.clear_intro_lines();
                    self.ng_visible = false;
                }
                9 => self.set_intro_lines(&[&self.wacky.line1.clone()]),
                11 => self.push_intro_line(&self.wacky.line2.clone()),
                12 => self.clear_intro_lines(),
                13 => self.push_intro_line("Friday"),
                14 => {
                    let word = if self.wacky.line1.eq_ignore_ascii_case("trending") {
                        "Nigth"
                    } else {
                        "Night"
                    };
                    self.push_intro_line(word);
                }
                15 => self.push_intro_line("Funkin"),
                16 => self.skip_intro(),
                _ => {}
            }
        } else {
            if self.rainbow_mode && (beat & 1) == 0 {
                self.hue_shift = (self.hue_shift + 0.125).rem_euclid(1.0);
            }
            self.logo_frame = 0;
            self.gf_dance_left = !self.gf_dance_left;
            self.gf_anim_time_s = 0.0;
        }
    }

    fn skip_intro(&mut self) {
        if !self.skipped_intro {
            self.skipped_intro = true;
            self.ng_visible = false;
            self.clear_intro_lines();
            self.flash_alpha = 1.0;
        }
    }

    fn set_intro_lines(&mut self, lines: &[&str]) {
        self.clear_intro_lines();
        for l in lines {
            self.push_intro_line(l);
        }
    }

    fn push_intro_line(&mut self, line: &str) {
        if self.intro_line_count < self.intro_lines.len() {
            self.intro_lines[self.intro_line_count].clear();
            let _ = write!(self.intro_lines[self.intro_line_count], "{}", line.trim());
            self.intro_line_count += 1;
        }
    }

    fn clear_intro_lines(&mut self) {
        self.intro_line_count = 0;
        for line in &mut self.intro_lines {
            line.clear();
        }
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        // Solid black backdrop.
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );

        let hsv = [self.hue_shift, 0.0, 0.0];

        if !self.skipped_intro {
            for (idx, line) in self
                .intro_lines
                .iter()
                .take(self.intro_line_count)
                .enumerate()
            {
                let text = line.as_str();
                if !text.is_empty() {
                    font.draw_str(
                        batcher,
                        font_tex,
                        text,
                        VIRTUAL_WIDTH * 0.5,
                        180.0 + (idx as f32) * 64.0,
                        &TextStyle {
                            scale: 1.9,
                            color: [1.0, 1.0, 1.0, 1.0],
                            align: TextAlign::Center,
                            ..Default::default()
                        },
                    );
                }
            }
            if self.ng_visible {
                if let Some((tex, sz)) = self.ng_tex {
                    let w = sz[0] * 0.8;
                    let h = sz[1] * 0.8;
                    batcher.push(
                        tex,
                        BlendMode::Normal,
                        FilterMode::Linear,
                        SpriteInstance::new([(VIRTUAL_WIDTH - w) * 0.5, VIRTUAL_HEIGHT * 0.52], [w, h]),
                    );
                }
            }
        } else {
            if let Some((tex, sz)) = self.logo_tex {
                if let Some(frame) = self.logo_frames.get(self.logo_frame) {
                    draw_menu_sparrow_frame(
                        batcher,
                        tex,
                        sz,
                        frame,
                        [-150.0, -100.0],
                        [1.0, 1.0],
                        false,
                        [1.0, 1.0, 1.0, 1.0],
                        hsv,
                        FilterMode::Linear,
                    );
                }
            }
            if let Some((tex, sz)) = self.gf_tex {
                let slice = if self.gf_dance_left {
                    &self.gf_left_frames
                } else {
                    &self.gf_right_frames
                };
                if !slice.is_empty() {
                    let idx = ((self.gf_anim_time_s * 24.0).floor() as usize).min(slice.len() - 1);
                    if let Some(frame) = slice.get(idx) {
                        draw_menu_sparrow_frame(
                            batcher,
                            tex,
                            sz,
                            frame,
                            [VIRTUAL_WIDTH * 0.4, VIRTUAL_HEIGHT * 0.07],
                            [1.0, 1.0],
                            false,
                            [1.0, 1.0, 1.0, 1.0],
                            hsv,
                            FilterMode::Linear,
                        );
                    }
                }
            }

            if !self.title_text_parts.is_empty() {
                let (b_orig, _) = self.title_text_bounds;
                let base_x = 100.0 - b_orig[0];
                let base_y = VIRTUAL_HEIGHT * 0.8 - b_orig[1];
                for part in &self.title_text_parts {
                    let Some(&(tex, _)) = self.title_text_textures.get(part.spritemap_index) else {
                        continue;
                    };
                    let (w, h) = if part.rotated {
                        (part.size[1], part.size[0])
                    } else {
                        (part.size[0], part.size[1])
                    };
                    let m = part.transform;
                    let tx = base_x + m.translation.x;
                    let ty = base_y + m.translation.y;
                    let flags = SPRITE_FLAG_AFFINE
                        | if part.rotated {
                            SPRITE_FLAG_ROTATED_90_CCW
                        } else {
                            0
                        };
                    let inst = SpriteInstance::new(
                        [tx, ty],
                        [m.matrix2.x_axis.x * w, m.matrix2.x_axis.y * w],
                    )
                    .with_origin([m.matrix2.y_axis.x * h, m.matrix2.y_axis.y * h])
                    .with_uv_rect([
                        part.uv_rect[0],
                        part.uv_rect[1],
                        part.uv_rect[0] + part.uv_rect[2],
                        part.uv_rect[1] + part.uv_rect[3],
                    ])
                    .with_color(part.color)
                    .with_hsv_shift(hsv)
                    .with_flags(flags);
                    batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
                }
            } else {
                font.draw_str(
                    batcher,
                    font_tex,
                    "PRESS ENTER TO BEGIN",
                    VIRTUAL_WIDTH * 0.5,
                    VIRTUAL_HEIGHT * 0.82,
                    &TextStyle {
                        scale: 1.4,
                        color: [1.0, 1.0, 1.0, 1.0],
                        align: TextAlign::Center,
                        ..Default::default()
                    },
                );
            }
        }

        if self.flash_alpha > 0.001 {
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([1.0, 1.0, 1.0, self.flash_alpha]),
            );
        }
    }
}

struct LoadedTitleTextAtlas {
    atlas: Option<AnimateAtlas>,
    textures: Vec<(TextureHandle, [f32; 2])>,
    bounds: ([f32; 2], [f32; 2]),
}

impl Default for LoadedTitleTextAtlas {
    fn default() -> Self {
        Self {
            atlas: None,
            textures: Vec::new(),
            bounds: ([0.0, 0.0], [100.0, 100.0]),
        }
    }
}

fn load_title_text_atlas(assets: &AssetLocator, gpu: &mut GpuContext) -> LoadedTitleTextAtlas {
    let dir = assets.animate_atlas_dir("title-screen-text");
    let Ok(anim_json) = fs::read_to_string(dir.join("Animation.json")) else {
        return LoadedTitleTextAtlas::default();
    };
    let sm_json_path = dir.join("spritemap1.json");
    let sm_png_path = dir.join("spritemap1.png");
    let (Ok(sm_json), Ok(png_bytes)) = (fs::read_to_string(&sm_json_path), fs::read(&sm_png_path))
    else {
        return LoadedTitleTextAtlas::default();
    };
    let Ok(tex) = gpu.from_png_bytes(Some("title-screen-text"), &png_bytes) else {
        return LoadedTitleTextAtlas::default();
    };
    let Some((tw, th)) = gpu.texture_size(tex) else {
        return LoadedTitleTextAtlas::default();
    };
    let Ok(atlas) = AnimateAtlas::from_json_strs(&anim_json, &[sm_json.as_str()]) else {
        return LoadedTitleTextAtlas::default();
    };
    let bounds = atlas.full_timeline_bounds();
    LoadedTitleTextAtlas {
        atlas: Some(atlas),
        textures: vec![(tex, [tw as f32, th as f32])],
        bounds,
    }
}

fn load_random_wacky(assets: &AssetLocator) -> WackyPair {
    let path = assets.root().join("data").join("introText.txt");
    if let Ok(txt) = fs::read_to_string(path) {
        let pairs: Vec<WackyPair> = txt
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                let (a, b) = trimmed.split_once("--")?;
                Some(WackyPair {
                    line1: a.trim().to_owned(),
                    line2: b.trim().to_owned(),
                })
            })
            .collect();
        if !pairs.is_empty() {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos() as usize);
            let idx = secs % pairs.len();
            return WackyPair {
                line1: pairs[idx].line1.clone(),
                line2: pairs[idx].line2.clone(),
            };
        }
    }
    WackyPair {
        line1: "fnfrs".to_owned(),
        line2: "v-slice in rust".to_owned(),
    }
}
