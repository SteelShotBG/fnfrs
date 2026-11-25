//! V-Slice `NoteStyle` GPU texture & Sparrow atlas loader (`NoteStyle.hx`)
//! with exact HaxeFlixel `centerOffsets()` / `frameX` / `rotated` sprite draw helpers.

use std::fs;

use fnfrs_core::constants::{
    DEFAULT_NOTE_STYLE, INITIAL_OFFSET, KEY_COUNT, NUDGE, STRUMLINE_SIZE,
};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::notestyle::NoteStyleData;
use fnfrs_render::{
    AtlasFrame, BlendMode, FilterMode, GpuContext, SparrowAtlas, SpriteBatcher, SpriteInstance,
    TextureHandle, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y, SPRITE_FLAG_PIXEL_SNAP,
    SPRITE_FLAG_ROTATED_90_CW,
};

use crate::input::NoteDirection;
use crate::strumline::StrumlineVisualStyle;

/// Loaded GPU texture + parsed SparrowV2 atlas pair.
#[derive(Debug, Clone)]
pub struct LoadedSparrow {
    pub texture: TextureHandle,
    pub tex_size: [f32; 2],
    pub atlas: SparrowAtlas,
}

/// Draw specification matching HaxeFlixel `FlxSprite.drawFrameComplex` with
/// trimmed `frameX`/`frameY`/`frameWidth`/`frameHeight` and `rotated="true"`.
#[derive(Debug, Clone, Copy)]
pub struct SparrowDrawParams {
    pub texture: TextureHandle,
    pub tex_size: [f32; 2],
    pub pos: [f32; 2],
    pub scale: f32,
    pub hitbox_size: [f32; 2],
    /// Manual draw offset subtracted from `pos` (e.g. `NoteSplash`'s `width * 0.3`).
    pub extra_offset: [f32; 2],
    pub color: [f32; 4],
    pub hsv_shift: [f32; 3],
    pub blend: BlendMode,
    pub filter: FilterMode,
    pub pixel_snap: bool,
}

/// Queues one Sparrow `AtlasFrame` into `batcher` using HaxeFlixel `centerOffsets()`
/// centering inside `params.hitbox_size`, with zero heap allocations.
pub fn push_sparrow_frame(
    batcher: &mut SpriteBatcher,
    frame: &AtlasFrame,
    params: &SparrowDrawParams,
) {
    let src = frame.source_size();
    let trimmed = frame.upright_trimmed_size();
    let scale = params.scale;

    let untrimmed_x = params.pos[0] - params.extra_offset[0]
        + (params.hitbox_size[0] - src[0] * scale) * 0.5;
    let untrimmed_y = params.pos[1] - params.extra_offset[1]
        + (params.hitbox_size[1] - src[1] * scale) * 0.5;

    let draw_x = untrimmed_x + (-frame.frame_x) * scale;
    let draw_y = untrimmed_y + (-frame.frame_y) * scale;
    let draw_w = trimmed[0] * scale;
    let draw_h = trimmed[1] * scale;

    let mut flags = 0u32;
    if frame.rotated {
        flags |= SPRITE_FLAG_ROTATED_90_CW;
    }
    if frame.flip_x {
        flags |= SPRITE_FLAG_FLIP_X;
    }
    if frame.flip_y {
        flags |= SPRITE_FLAG_FLIP_Y;
    }
    if params.pixel_snap {
        flags |= SPRITE_FLAG_PIXEL_SNAP;
    }

    let inst = SpriteInstance::new([draw_x, draw_y], [draw_w, draw_h])
        .with_uv_rect(frame.uv_bounds(params.tex_size[0], params.tex_size[1]))
        .with_color(params.color)
        .with_hsv_shift(params.hsv_shift)
        .with_flags(flags);

    batcher.push(params.texture, params.blend, params.filter, inst);
}

/// Pre-indexed GPU textures and animation frames for a V-Slice `NoteStyle`.
#[derive(Debug, Clone)]
pub struct NoteStyleAssets {
    pub style_id: String,

    pub strum_tex: Option<(TextureHandle, [f32; 2])>,
    pub strum_scale: f32,
    pub strum_offsets: [f32; 2],
    pub strum_pixel: bool,
    pub strum_hitbox: [f32; 2],
    pub strum_static: [Vec<AtlasFrame>; KEY_COUNT],
    pub strum_press: [Vec<AtlasFrame>; KEY_COUNT],
    pub strum_confirm: [Vec<AtlasFrame>; KEY_COUNT],
    pub strum_confirm_hold: [Vec<AtlasFrame>; KEY_COUNT],

    pub note_tex: Option<(TextureHandle, [f32; 2])>,
    pub note_scale: f32,
    pub note_offsets: [f32; 2],
    pub note_pixel: bool,
    pub note_hitbox: [[f32; 2]; KEY_COUNT],
    pub note_frames: [Vec<AtlasFrame>; KEY_COUNT],

    pub hold_tex: Option<(TextureHandle, [f32; 2])>,
    pub hold_scale: f32,
    pub hold_offsets: [f32; 2],
    pub hold_pixel: bool,

    pub splash_tex: Option<(TextureHandle, [f32; 2])>,
    pub splash_scale: f32,
    pub splash_offsets: [f32; 2],
    pub splash_alpha: f32,
    pub splash_fps: f32,
    pub splash_pixel: bool,
    pub splash_blend: BlendMode,
    pub splash_variants: [Vec<Vec<AtlasFrame>>; KEY_COUNT],

    pub cover_tex: [Option<(TextureHandle, [f32; 2])>; KEY_COUNT],
    pub cover_scale: f32,
    pub cover_offsets: [f32; 2],
    pub cover_pixel: bool,
    pub cover_hitbox: [f32; 2],
    pub cover_start: [Vec<AtlasFrame>; KEY_COUNT],
    pub cover_loop: [Vec<AtlasFrame>; KEY_COUNT],
    pub cover_end: [Vec<AtlasFrame>; KEY_COUNT],

    pub health_bar_tex: Option<(TextureHandle, [f32; 2])>,
}

impl Default for NoteStyleAssets {
    fn default() -> Self {
        Self::empty(DEFAULT_NOTE_STYLE)
    }
}

impl NoteStyleAssets {
    /// Creates an empty asset container (geometric fallback mode when `assets/` is absent).
    #[must_use]
    pub fn empty(style_id: &str) -> Self {
        Self {
            style_id: style_id.to_owned(),
            strum_tex: None,
            strum_scale: 0.7,
            strum_offsets: [0.0, 0.0],
            strum_pixel: false,
            strum_hitbox: [STRUMLINE_SIZE, STRUMLINE_SIZE],
            strum_static: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            strum_press: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            strum_confirm: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            strum_confirm_hold: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            note_tex: None,
            note_scale: 0.7,
            note_offsets: [0.0, 0.0],
            note_pixel: false,
            note_hitbox: [[STRUMLINE_SIZE, STRUMLINE_SIZE]; KEY_COUNT],
            note_frames: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            hold_tex: None,
            hold_scale: 0.7,
            hold_offsets: [0.0, 0.0],
            hold_pixel: false,
            splash_tex: None,
            splash_scale: 1.0,
            splash_offsets: [25.0, -5.0],
            splash_alpha: 0.8,
            splash_fps: 24.0,
            splash_pixel: false,
            splash_blend: BlendMode::Add,
            splash_variants: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cover_tex: [None; KEY_COUNT],
            cover_scale: 1.0,
            cover_offsets: [0.0, 0.0],
            cover_pixel: false,
            cover_hitbox: [300.0, 400.0],
            cover_start: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cover_loop: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cover_end: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            health_bar_tex: None,
        }
    }

    /// Loads `data/notestyles/<style_id>.json` and all available V-Slice sheets from `assets`.
    /// Missing files degrade gracefully to `None`.
    pub fn load_from_assets(assets: &AssetLocator, gpu: &mut GpuContext, style_id: &str) -> Self {
        let mut out = Self::empty(style_id);

        let health_bar_path = assets.image("healthBar");
        if let Some(tex) = upload_png_from_path(gpu, Some("healthBar"), &health_bar_path) {
            out.health_bar_tex = Some(tex);
        }

        let json_key = format!("notestyles/{style_id}");
        let json_path = assets.data_json(&json_key);
        let Ok(raw_json) = fs::read_to_string(&json_path) else {
            // Try loading standard `funkin` shared sheets directly if JSON missing.
            out.load_default_sheets(assets, gpu);
            return out;
        };

        let Ok(data) = serde_json::from_str::<NoteStyleData>(&raw_json) else {
            out.load_default_sheets(assets, gpu);
            return out;
        };

        // 1. Strumline receptors.
        if let Some(strum_cfg) = &data.assets.note_strumline {
            out.strum_scale = strum_cfg.scale;
            out.strum_offsets = strum_cfg.offsets;
            out.strum_pixel = strum_cfg.is_pixel;
            let key = strum_cfg.asset_path.as_deref().unwrap_or("shared:noteStrumline");
            if let Some(sparrow) = load_sparrow_sheet(assets, gpu, key) {
                out.strum_tex = Some((sparrow.texture, sparrow.tex_size));
                if let Some(d) = &strum_cfg.data {
                    let prefixes = [
                        (
                            &d.left_static.prefix,
                            &d.left_press.prefix,
                            &d.left_confirm.prefix,
                            &d.left_confirm_hold.prefix,
                        ),
                        (
                            &d.down_static.prefix,
                            &d.down_press.prefix,
                            &d.down_confirm.prefix,
                            &d.down_confirm_hold.prefix,
                        ),
                        (
                            &d.up_static.prefix,
                            &d.up_press.prefix,
                            &d.up_confirm.prefix,
                            &d.up_confirm_hold.prefix,
                        ),
                        (
                            &d.right_static.prefix,
                            &d.right_press.prefix,
                            &d.right_confirm.prefix,
                            &d.right_confirm_hold.prefix,
                        ),
                    ];
                    for (i, (st, pr, cf, ch)) in prefixes.iter().enumerate() {
                        if let Some(p) = st.as_deref() {
                            out.strum_static[i] = collect_prefix(&sparrow.atlas, p);
                        }
                        if let Some(p) = pr.as_deref() {
                            out.strum_press[i] = collect_prefix(&sparrow.atlas, p);
                        }
                        if let Some(p) = cf.as_deref() {
                            out.strum_confirm[i] = collect_prefix(&sparrow.atlas, p);
                        }
                        if let Some(p) = ch.as_deref() {
                            out.strum_confirm_hold[i] = collect_prefix(&sparrow.atlas, p);
                        }
                    }
                    if let Some(first_static) = out.strum_static[0].first() {
                        let src = first_static.source_size();
                        out.strum_hitbox = [src[0] * out.strum_scale, src[1] * out.strum_scale];
                    }
                }
            }
        }

        // 2. Note heads.
        if let Some(note_cfg) = &data.assets.note {
            out.note_scale = note_cfg.scale;
            out.note_offsets = note_cfg.offsets;
            out.note_pixel = note_cfg.is_pixel;
            let key = note_cfg.asset_path.as_deref().unwrap_or("shared:notes");
            if let Some(sparrow) = load_sparrow_sheet(assets, gpu, key) {
                out.note_tex = Some((sparrow.texture, sparrow.tex_size));
                if let Some(d) = &note_cfg.data {
                    let prefixes = [
                        d.left.prefix.as_deref(),
                        d.down.prefix.as_deref(),
                        d.up.prefix.as_deref(),
                        d.right.prefix.as_deref(),
                    ];
                    for (i, pref_opt) in prefixes.iter().enumerate() {
                        if let Some(p) = pref_opt {
                            out.note_frames[i] = collect_prefix(&sparrow.atlas, p);
                            if let Some(f) = out.note_frames[i].first() {
                                let src = f.source_size();
                                out.note_hitbox[i] =
                                    [src[0] * out.note_scale, src[1] * out.note_scale];
                            }
                        }
                    }
                }
            }
        }

        // 3. Sustain hold strip.
        if let Some(hold_cfg) = &data.assets.hold_note {
            out.hold_scale = hold_cfg.scale;
            out.hold_offsets = hold_cfg.offsets;
            out.hold_pixel = hold_cfg.is_pixel;
            let key = hold_cfg.asset_path.as_deref().unwrap_or("NOTE_hold_assets");
            out.hold_tex = upload_png_from_path(gpu, Some(key), &assets.image(key));
        }

        // 4. Note splashes.
        if let Some(splash_cfg) = &data.assets.note_splash {
            out.splash_scale = splash_cfg.scale;
            out.splash_offsets = splash_cfg.offsets;
            out.splash_alpha = splash_cfg.alpha;
            out.splash_pixel = splash_cfg.is_pixel;
            if let Some(d) = &splash_cfg.data {
                out.splash_fps = d.framerate_default.max(1) as f32;
                out.splash_blend = if d.blend_mode.eq_ignore_ascii_case("add") {
                    BlendMode::Add
                } else if d.blend_mode.eq_ignore_ascii_case("screen") {
                    BlendMode::Screen
                } else {
                    BlendMode::Normal
                };
                if d.enabled {
                    let key = splash_cfg.asset_path.as_deref().unwrap_or("shared:noteSplashes");
                    if let Some(sparrow) = load_sparrow_sheet(assets, gpu, key) {
                        out.splash_tex = Some((sparrow.texture, sparrow.tex_size));
                        let lists = [
                            &d.left_splashes,
                            &d.down_splashes,
                            &d.up_splashes,
                            &d.right_splashes,
                        ];
                        for (i, list) in lists.iter().enumerate() {
                            for anim in *list {
                                if let Some(p) = anim.prefix.as_deref() {
                                    let frames = collect_prefix(&sparrow.atlas, p);
                                    if !frames.is_empty() {
                                        out.splash_variants[i].push(frames);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 5. Hold note covers.
        if let Some(cover_cfg) = &data.assets.hold_note_cover {
            out.cover_scale = cover_cfg.scale;
            out.cover_offsets = cover_cfg.offsets;
            out.cover_pixel = cover_cfg.is_pixel;
            if let Some(d) = &cover_cfg.data {
                if d.enabled {
                    let dirs = [&d.left, &d.down, &d.up, &d.right];
                    for (i, dir_opt) in dirs.into_iter().enumerate() {
                        let Some(dir_cfg) = dir_opt.as_ref() else {
                            continue;
                        };
                        let Some(key) = dir_cfg.asset_path.as_deref() else {
                            continue;
                        };
                        if let Some(sparrow) = load_sparrow_sheet(assets, gpu, key) {
                            out.cover_tex[i] = Some((sparrow.texture, sparrow.tex_size));
                            if let Some(p) = dir_cfg.start.as_ref().and_then(|a| a.prefix.as_deref())
                            {
                                out.cover_start[i] = collect_prefix(&sparrow.atlas, p);
                            }
                            if let Some(p) = dir_cfg.hold.as_ref().and_then(|a| a.prefix.as_deref())
                            {
                                out.cover_loop[i] = collect_prefix(&sparrow.atlas, p);
                            }
                            if let Some(p) = dir_cfg.end.as_ref().and_then(|a| a.prefix.as_deref()) {
                                out.cover_end[i] = collect_prefix(&sparrow.atlas, p);
                            }
                            if let Some(f) = out.cover_start[i]
                                .first()
                                .or_else(|| out.cover_loop[i].first())
                            {
                                let src = f.source_size();
                                out.cover_hitbox =
                                    [src[0] * out.cover_scale, src[1] * out.cover_scale];
                            }
                        }
                    }
                }
            }
        }

        out
    }

    fn load_default_sheets(&mut self, assets: &AssetLocator, gpu: &mut GpuContext) {
        self.hold_tex =
            upload_png_from_path(gpu, Some("NOTE_hold_assets"), &assets.image("NOTE_hold_assets"));
    }

    /// Returns `true` if `noteStrumline` Sparrow frames were loaded.
    #[inline]
    #[must_use]
    pub fn has_sparrow_strum(&self) -> bool {
        self.strum_tex.is_some() && self.strum_static.iter().all(|v| !v.is_empty())
    }

    /// Returns `true` if `notes` Sparrow frames were loaded.
    #[inline]
    #[must_use]
    pub fn has_sparrow_notes(&self) -> bool {
        self.note_tex.is_some() && self.note_frames.iter().all(|v| !v.is_empty())
    }

    /// Returns `true` if `noteSplashes` frames were loaded.
    #[inline]
    #[must_use]
    pub fn has_sparrow_splashes(&self) -> bool {
        self.splash_tex.is_some() && self.splash_variants.iter().any(|v| !v.is_empty())
    }

    /// Returns `true` if hold cover frames were loaded for `dir`.
    #[inline]
    #[must_use]
    pub fn has_cover_for(&self, dir: NoteDirection) -> bool {
        let i = dir.as_index();
        self.cover_tex[i].is_some()
            && (!self.cover_start[i].is_empty() || !self.cover_loop[i].is_empty())
    }

    /// Populates a [`StrumlineVisualStyle`] matching these loaded assets.
    #[must_use]
    pub fn to_visual_style(&self, white_texture: TextureHandle) -> StrumlineVisualStyle {
        let mut style = StrumlineVisualStyle::fallback(white_texture);
        if let Some((tex, size)) = self.hold_tex {
            style.hold_texture = Some(tex);
            style.hold_tex_size = size;
            style.hold_zoom = self.hold_scale;
            style.hold_offsets = self.hold_offsets;
        }
        style.is_pixel = self.strum_pixel || self.note_pixel || self.hold_pixel;
        style.use_sparrow_offsets = self.has_sparrow_strum();
        style.note_nudge = if self.has_sparrow_notes() { NUDGE } else { 0.0 };
        if let Some(confirm) = self.strum_confirm.first().filter(|v| !v.is_empty()) {
            style.confirm_frames = confirm.len();
        }
        if let Some(first_splash) = self
            .splash_variants
            .iter()
            .find_map(|v| v.first())
            .filter(|v| !v.is_empty())
        {
            style.splash_frames = first_splash.len();
        }
        if let Some(start) = self.cover_start.iter().find(|v| !v.is_empty()) {
            style.cover_start_frames = start.len();
        }
        if let Some(loop_f) = self.cover_loop.iter().find(|v| !v.is_empty()) {
            style.cover_loop_frames = loop_f.len();
        }
        if let Some(end) = self.cover_end.iter().find(|v| !v.is_empty()) {
            style.cover_end_frames = end.len();
        }
        style.cover_hitbox = self.cover_hitbox;
        style.visual_initial_offset = if style.use_sparrow_offsets {
            INITIAL_OFFSET
        } else {
            0.0
        };
        style
    }
}

fn upload_png_from_path(
    gpu: &mut GpuContext,
    label: Option<&str>,
    path: &std::path::Path,
) -> Option<(TextureHandle, [f32; 2])> {
    let bytes = fs::read(path).ok()?;
    let tex = gpu.from_png_bytes(label, &bytes).ok()?;
    let (w, h) = gpu.texture_size(tex)?;
    Some((tex, [w as f32, h as f32]))
}

fn load_sparrow_sheet(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
) -> Option<LoadedSparrow> {
    let png_path = assets.image(key);
    let xml_path = assets.sparrow_xml(key);
    let (texture, tex_size) = upload_png_from_path(gpu, Some(key), &png_path)?;
    let xml = fs::read_to_string(&xml_path).ok()?;
    let atlas = SparrowAtlas::from_xml(&xml).ok()?;
    Some(LoadedSparrow {
        texture,
        tex_size,
        atlas,
    })
}

fn collect_prefix(atlas: &SparrowAtlas, prefix: &str) -> Vec<AtlasFrame> {
    atlas.frames_by_prefix(prefix).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparrow_frame_trim_and_center_offset_calculation() {
        let frame = AtlasFrame {
            name: "holdCoverBlue0001".to_owned(),
            x: 407,
            y: 242,
            width: 108,
            height: 138,
            frame_x: -94.0,
            frame_y: -94.0,
            frame_width: 300.0,
            frame_height: 400.0,
            rotated: true,
            flip_x: false,
            flip_y: false,
        };

        assert!((frame.upright_trimmed_size()[0] - 138.0).abs() < 1e-6);
        assert!((frame.upright_trimmed_size()[1] - 108.0).abs() < 1e-6);

        let uv = frame.uv_bounds(599.0, 591.0);
        assert!((uv[0] - (407.0 / 599.0)).abs() < 1e-6);
        assert!((uv[1] - (242.0 / 591.0)).abs() < 1e-6);
        assert!((uv[2] - (515.0 / 599.0)).abs() < 1e-6);
        assert!((uv[3] - (380.0 / 591.0)).abs() < 1e-6);
    }
}
