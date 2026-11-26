//! V-Slice Playable Character Select (`CharSelectSubState.hx`).

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AnimateAtlas, AtlasFont, AtlasTextStyle, BitmapFont, BlendMode, EvaluatedPart, FilterMode,
    GpuContext, SpriteBatcher, SpriteInstance, TextAlign, TextureHandle, SPRITE_FLAG_AFFINE,
    SPRITE_FLAG_ROTATED_90_CCW,
};

use super::{load_menu_atlas_font, load_menu_image, MenuCommand, MenuInputState};
use crate::character::{upload_png_path};
use std::fs;

#[derive(Debug)]
struct ChillPreview {
    atlas: Option<AnimateAtlas>,
    textures: Vec<(TextureHandle, [f32; 2])>,
    parts: Vec<EvaluatedPart>,
    bounds: ([f32; 2], [f32; 2]),
}

impl ChillPreview {
    fn load(assets: &AssetLocator, gpu: &mut GpuContext, key: &str) -> Self {
        let dir = assets.animate_atlas_dir(key);
        let Ok(anim_json) = fs::read_to_string(dir.join("Animation.json")) else {
            return Self::empty();
        };
        let Ok(sm_json) = fs::read_to_string(dir.join("spritemap1.json")) else {
            return Self::empty();
        };
        let Ok(atlas) = AnimateAtlas::from_json_strs(&anim_json, &[&sm_json]) else {
            return Self::empty();
        };
        let mut textures = Vec::with_capacity(atlas.spritemaps().len());
        for sm in atlas.spritemaps() {
            if let Some(t) = upload_png_path(gpu, Some(&sm.image), &dir.join(&sm.image)) {
                textures.push(t);
            }
        }
        let bounds = atlas.full_timeline_bounds();
        Self {
            atlas: Some(atlas),
            textures,
            parts: Vec::with_capacity(64),
            bounds,
        }
    }

    fn empty() -> Self {
        Self {
            atlas: None,
            textures: Vec::new(),
            parts: Vec::with_capacity(0),
            bounds: ([0.0, 0.0], [1.0, 1.0]),
        }
    }

    fn update(&mut self, time_s: f32) {
        let Some(atlas) = &self.atlas else {
            return;
        };
        let total = atlas.total_frames().max(1);
        let frame = ((time_s * atlas.frame_rate()).floor() as u32) % total;
        atlas.evaluate_frame(frame, &mut self.parts);
    }

    fn draw(&self, batcher: &mut SpriteBatcher, origin: [f32; 2], scale: f32) {
        let (b_orig, _) = self.bounds;
        for part in &self.parts {
            let Some(&(tex, _)) = self.textures.get(part.spritemap_index) else {
                continue;
            };
            let (w, h) = if part.rotated {
                (part.size[1], part.size[0])
            } else {
                (part.size[0], part.size[1])
            };
            let m = part.transform;
            let tx = origin[0] + (m.translation.x - b_orig[0]) * scale;
            let ty = origin[1] + (m.translation.y - b_orig[1]) * scale;
            let flags = SPRITE_FLAG_AFFINE
                | if part.rotated {
                    SPRITE_FLAG_ROTATED_90_CCW
                } else {
                    0
                };
            let inst = SpriteInstance::new(
                [tx, ty],
                [m.matrix2.x_axis.x * w * scale, m.matrix2.x_axis.y * w * scale],
            )
            .with_origin([m.matrix2.y_axis.x * h * scale, m.matrix2.y_axis.y * h * scale])
            .with_uv_rect([
                part.uv_rect[0],
                part.uv_rect[1],
                part.uv_rect[0] + part.uv_rect[2],
                part.uv_rect[1] + part.uv_rect[3],
            ])
            .with_color(part.color)
            .with_flags(flags);
            batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
        }
    }
}

/// Interactive Character Select (`bf` / `pico`).
#[derive(Debug)]
pub struct CharSelectScreenState {
    white_tex: TextureHandle,
    bg_tex: Option<(TextureHandle, [f32; 2])>,
    curtains_tex: Option<(TextureHandle, [f32; 2])>,
    bf_nametag: Option<(TextureHandle, [f32; 2])>,
    atlas_font: AtlasFont,
    previews: [ChillPreview; 2],
    companions: [ChillPreview; 2],
    pub selected: usize,
    elapsed_s: f32,
}

impl CharSelectScreenState {
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext, current_char: &str) -> Self {
        let bg_tex = load_menu_image(assets, gpu, "charSelect/charSelectBG");
        let curtains_tex = load_menu_image(assets, gpu, "charSelect/curtains");
        let bf_nametag = load_menu_image(assets, gpu, "charSelect/boyfriendNametag");
        let atlas_font = load_menu_atlas_font(assets, gpu, true);
        let previews = [
            ChillPreview::load(assets, gpu, "charSelect/bfChill"),
            ChillPreview::load(assets, gpu, "charSelect/picoChill"),
        ];
        let companions = [
            ChillPreview::load(assets, gpu, "charSelect/gfChill"),
            ChillPreview::load(assets, gpu, "charSelect/neneChill"),
        ];
        let selected = if current_char.eq_ignore_ascii_case("pico") {
            1
        } else {
            0
        };
        Self {
            white_tex: gpu.white_texture(),
            bg_tex,
            curtains_tex,
            bf_nametag,
            atlas_font,
            previews,
            companions,
            selected,
            elapsed_s: 0.0,
        }
    }

    #[must_use]
    pub fn selected_character(&self) -> &'static str {
        if self.selected == 1 {
            "pico"
        } else {
            "bf"
        }
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        self.elapsed_s += elapsed_s.clamp(0.0, 0.1);
        self.previews[self.selected].update(self.elapsed_s);
        self.companions[self.selected].update(self.elapsed_s);

        if input.back_pressed {
            return MenuCommand::EnterFreeplay;
        }
        if input.left_pressed || input.right_pressed {
            self.selected = 1 - self.selected;
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.accept_pressed {
            return MenuCommand::SavePreferencesChanged;
        }
        MenuCommand::None
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, _font: &BitmapFont, _font_tex: TextureHandle) {
        if let Some((tex, sz)) = self.bg_tex {
            let s = (VIRTUAL_WIDTH / sz[0].max(1.0)).max(VIRTUAL_HEIGHT / sz[1].max(1.0));
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([0.0, 0.0], [sz[0] * s, sz[1] * s]),
            );
        } else {
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([0.1, 0.08, 0.18, 1.0]),
            );
        }

        self.companions[self.selected].draw(batcher, [650.0, 210.0], 0.9);
        self.previews[self.selected].draw(batcher, [430.0, 260.0], 0.95);

        if let Some((tex, sz)) = self.curtains_tex {
            let s = VIRTUAL_WIDTH / sz[0].max(1.0);
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([0.0, 0.0], [sz[0] * s, sz[1] * s]),
            );
        }

        if self.selected == 0 {
            if let Some((nt, nsz)) = self.bf_nametag {
                batcher.push(
                    nt,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([VIRTUAL_WIDTH - nsz[0] - 48.0, 48.0], nsz),
                );
            }
        }
        if self.atlas_font.is_loaded() {
            let label = if self.selected == 1 { "PICO" } else { "BOYFRIEND" };
            if self.selected != 0 || self.bf_nametag.is_none() {
                self.atlas_font.draw_str(
                    batcher,
                    label,
                    [VIRTUAL_WIDTH - 64.0, 56.0],
                    &AtlasTextStyle {
                        scale: 0.85,
                        color: [1.0, 1.0, 1.0, 1.0],
                        align: TextAlign::Right,
                        time_s: self.elapsed_s,
                        animated: true,
                        ..Default::default()
                    },
                );
            }
            self.atlas_font.draw_str(
                batcher,
                "< CHOOSE YOUR CHARACTER >",
                [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT - 96.0],
                &AtlasTextStyle {
                    scale: 0.65,
                    color: [1.0, 0.94, 0.35, 1.0],
                    align: TextAlign::Center,
                    time_s: self.elapsed_s,
                    animated: true,
                    ..Default::default()
                },
            );
        }
    }
}
