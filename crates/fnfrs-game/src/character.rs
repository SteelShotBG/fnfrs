//! Runtime V-Slice character simulation, atlas loading (Sparrow, Packer,
//! AnimateAtlas, Multi-Sparrow, Multi-AnimateAtlas), and zero-allocation rendering
//! matching `funkin/play/character/BaseCharacter.hx` and its subclasses.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use fnfrs_core::constants::{KEY_COUNT, MS_PER_SEC, STEPS_PER_BEAT, VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::character::{
    AnimationData, CharacterData, CharacterRenderType, DeathData, HealthIconData,
};
use fnfrs_core::schema::stage::{StageDataCharacter, TextureAtlasData};
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BlendMode, EvaluatedPart, FilterMode, GpuContext, PackerAtlas,
    SparrowAtlas, SpriteBatcher, SpriteInstance, TextureHandle, SPRITE_FLAG_FLIP_X,
    SPRITE_FLAG_FLIP_Y, SPRITE_FLAG_ROTATED_90_CW,
};

use crate::input::NoteDirection;

const DEFAULT_ANIM_FPS: f32 = 24.0;
const INITIAL_EVAL_PARTS_CAP: usize = 64;

/// Role of a character in gameplay (`CharacterType` in `BaseCharacter.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CharacterKind {
    Bf,
    Dad,
    Gf,
    #[default]
    Other,
}

/// Minimal 2D affine matrix (`[a, b, c, d, tx, ty]`) mirroring `glam::Affine2`
/// without requiring a direct `glam` dependency in `fnfrs-game`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Affine2D {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine2D {
    pub(crate) const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    #[inline]
    #[must_use]
    pub(crate) const fn translation(tx: f32, ty: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx,
            ty,
        }
    }

    #[inline]
    #[must_use]
    pub(crate) const fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    #[inline]
    #[must_use]
    pub(crate) fn rotation(rad: f32) -> Self {
        let (sin, cos) = rad.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    #[inline]
    #[must_use]
    pub(crate) fn mul(self, rhs: Self) -> Self {
        Self {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            tx: self.a * rhs.tx + self.c * rhs.ty + self.tx,
            ty: self.b * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }

    #[inline]
    #[must_use]
    pub(crate) fn transform_point(self, x: f32, y: f32) -> [f32; 2] {
        [self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty]
    }
}

/// Maps world coordinates and size through `cam_pos` (camera scroll) and `cam_zoom`
/// centered on the `1280x720` virtual canvas (`FlxCamera` + `scrollFactor`).
#[inline]
#[must_use]
pub fn apply_camera_transform(
    world_pos: [f32; 2],
    world_size: [f32; 2],
    scroll_factor: [f32; 2],
    cam_pos: [f32; 2],
    cam_zoom: f32,
) -> ([f32; 2], [f32; 2]) {
    let half_w = VIRTUAL_WIDTH * 0.5;
    let half_h = VIRTUAL_HEIGHT * 0.5;
    let screen_x = world_pos[0] - cam_pos[0] * scroll_factor[0];
    let screen_y = world_pos[1] - cam_pos[1] * scroll_factor[1];
    let zoomed_x = (screen_x - half_w) * cam_zoom + half_w;
    let zoomed_y = (screen_y - half_h) * cam_zoom + half_h;
    (
        [zoomed_x, zoomed_y],
        [world_size[0] * cam_zoom, world_size[1] * cam_zoom],
    )
}

/// Loaded GPU texture + parsed atlas backend shared by characters and animated stage props.
#[derive(Debug)]
pub(crate) enum LoadedAtlasSource {
    Sparrow {
        texture: TextureHandle,
        tex_size: [f32; 2],
        atlas: SparrowAtlas,
    },
    Packer {
        texture: TextureHandle,
        tex_size: [f32; 2],
        atlas: PackerAtlas,
    },
    Animate {
        textures: Vec<(TextureHandle, [f32; 2])>,
        atlas: AnimateAtlas,
        bounds_origin: [f32; 2],
        bounds_size: [f32; 2],
    },
}

impl LoadedAtlasSource {
    /// Returns the nominal untrimmed frame size `[width, height]` of the first available frame.
    pub(crate) fn fallback_hitbox_size(&self) -> Option<[f32; 2]> {
        match self {
            Self::Sparrow { atlas, .. } => atlas.frames().first().map(AtlasFrame::source_size),
            Self::Packer { atlas, .. } => atlas.frames().first().map(AtlasFrame::source_size),
            Self::Animate { bounds_size, .. } => Some(*bounds_size),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnimateAnimMode {
    FrameLabel,
    Symbol,
}

#[derive(Debug, Clone)]
pub(crate) enum AnimFrames {
    Sheet {
        atlas_idx: usize,
        frames: Vec<AtlasFrame>,
    },
    Animate {
        atlas_idx: usize,
        mode: AnimateAnimMode,
        target_name: String,
        indices: Vec<u32>,
        total_frames: u32,
    },
}

impl AnimFrames {
    #[inline]
    #[must_use]
    pub(crate) fn frame_count(&self) -> usize {
        match self {
            Self::Sheet { frames, .. } => frames.len(),
            Self::Animate {
                indices,
                total_frames,
                ..
            } => {
                if indices.is_empty() {
                    *total_frames as usize
                } else {
                    indices.len()
                }
            }
        }
    }

    #[must_use]
    pub(crate) fn first_source_size(&self, atlases: &[LoadedAtlasSource]) -> Option<[f32; 2]> {
        match self {
            Self::Sheet { frames, .. } => frames.first().map(AtlasFrame::source_size),
            Self::Animate { atlas_idx, .. } => atlases
                .get(*atlas_idx)
                .and_then(LoadedAtlasSource::fallback_hitbox_size),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedAnim {
    pub name: String,
    pub offsets: [f32; 2],
    pub looped: bool,
    pub flip_x: bool,
    pub flip_y: bool,
    pub frame_rate: f32,
    pub frames: AnimFrames,
    pub hold_anim: Option<usize>,
    pub end_anim: Option<usize>,
    pub is_sing: bool,
    pub is_miss: bool,
    pub is_hold_suffix: bool,
    pub is_end_suffix: bool,
    pub is_idle_or_dance: bool,
    pub is_combo_or_drop: bool,
}

/// Pre-compiled animation bank and zero-allocation playback state shared by
/// [`BaseCharacter`] and animated [`crate::stage::StageProp`]s.
#[derive(Debug)]
pub(crate) struct AnimBank {
    pub atlases: Vec<LoadedAtlasSource>,
    pub anims: Vec<LoadedAnim>,
    pub anim_index: HashMap<String, usize>,
    pub evaluated_parts: Vec<EvaluatedPart>,
    pub current_anim: Option<usize>,
    pub current_frame: usize,
    pub anim_time_s: f32,
    pub finished: bool,
}

impl Default for AnimBank {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimBank {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            atlases: Vec::new(),
            anims: Vec::new(),
            anim_index: HashMap::new(),
            evaluated_parts: Vec::with_capacity(INITIAL_EVAL_PARTS_CAP),
            current_anim: None,
            current_frame: 0,
            anim_time_s: 0.0,
            finished: false,
        }
    }

    pub(crate) fn finalize_links(&mut self) {
        self.anim_index.clear();
        for (idx, anim) in self.anims.iter().enumerate() {
            self.anim_index.entry(anim.name.clone()).or_insert(idx);
        }

        let mut stack_buf = [0u8; 128];
        for idx in 0..self.anims.len() {
            let name = self.anims[idx].name.as_str();
            let hold_anim = format_stack_join(&mut stack_buf, name, "-hold")
                .and_then(|s| self.anim_index.get(s).copied());
            let base = name.strip_suffix("-hold").unwrap_or(name);
            let end_anim = format_stack_join(&mut stack_buf, base, "-end")
                .and_then(|s| self.anim_index.get(s).copied());
            self.anims[idx].hold_anim = hold_anim;
            self.anims[idx].end_anim = end_anim;
        }
    }

    #[inline]
    #[must_use]
    pub(crate) fn current_name(&self) -> &str {
        self.current_anim
            .and_then(|idx| self.anims.get(idx))
            .map_or("", |a| a.name.as_str())
    }

    #[inline]
    #[must_use]
    pub(crate) fn current_offsets(&self) -> [f32; 2] {
        self.current_anim
            .and_then(|idx| self.anims.get(idx))
            .map_or([0.0, 0.0], |a| a.offsets)
    }

    #[inline]
    #[must_use]
    pub(crate) fn current_anim_meta(&self) -> Option<&LoadedAnim> {
        self.current_anim.and_then(|idx| self.anims.get(idx))
    }

    #[must_use]
    pub(crate) fn resolve_with_fallback(&self, name: &str) -> Option<usize> {
        if let Some(&idx) = self.anim_index.get(name) {
            return Some(idx);
        }
        let mut candidate = name;
        while let Some(pos) = candidate.rfind('-') {
            candidate = &candidate[..pos];
            if let Some(&idx) = self.anim_index.get(candidate) {
                return Some(idx);
            }
        }
        None
    }

    pub(crate) fn play_index(&mut self, anim_idx: usize, restart: bool) -> bool {
        let Some(anim) = self.anims.get(anim_idx) else {
            return false;
        };
        if !restart && self.current_anim == Some(anim_idx) && !self.finished {
            return false;
        }
        let count = anim.frames.frame_count();
        self.current_anim = Some(anim_idx);
        self.current_frame = 0;
        self.anim_time_s = 0.0;
        self.finished = count == 0;
        self.refresh_animate_parts();
        true
    }

    pub(crate) fn play_name(&mut self, name: &str, restart: bool) -> bool {
        let Some(idx) = self.resolve_with_fallback(name) else {
            return false;
        };
        self.play_index(idx, restart)
    }

    /// Advances animation timing with zero heap allocations.
    /// Returns `true` if the animation transitioned to `finished` on this tick.
    pub(crate) fn update(&mut self, elapsed_s: f32) -> bool {
        let Some(anim_idx) = self.current_anim else {
            return false;
        };
        let Some(anim) = self.anims.get(anim_idx) else {
            return false;
        };
        if self.finished {
            return false;
        }

        let count = anim.frames.frame_count();
        if count == 0 {
            self.finished = true;
            return true;
        }

        let fps = if anim.frame_rate > 0.0 {
            anim.frame_rate
        } else {
            DEFAULT_ANIM_FPS
        };
        self.anim_time_s += elapsed_s.max(0.0);
        let raw_frame = (self.anim_time_s * fps).floor() as usize;

        let prev_frame = self.current_frame;
        let mut just_finished = false;

        if anim.looped {
            self.current_frame = raw_frame % count;
        } else if raw_frame >= count {
            self.current_frame = count - 1;
            self.finished = true;
            just_finished = true;
        } else {
            self.current_frame = raw_frame;
        }

        if self.current_frame != prev_frame {
            self.refresh_animate_parts();
        }

        just_finished
    }

    pub(crate) fn refresh_animate_parts(&mut self) {
        self.evaluated_parts.clear();
        let Some(anim_idx) = self.current_anim else {
            return;
        };
        let Some(anim) = self.anims.get(anim_idx) else {
            return;
        };
        if let AnimFrames::Animate {
            atlas_idx,
            mode,
            target_name,
            indices,
            total_frames,
        } = &anim.frames
        {
            let Some(LoadedAtlasSource::Animate { atlas, .. }) = self.atlases.get(*atlas_idx) else {
                return;
            };
            let local_frame = if indices.is_empty() {
                (self.current_frame as u32).min(total_frames.saturating_sub(1))
            } else {
                indices[self.current_frame.min(indices.len() - 1)]
            };
            match mode {
                AnimateAnimMode::FrameLabel => {
                    atlas.evaluate_label(target_name, local_frame, &mut self.evaluated_parts);
                }
                AnimateAnimMode::Symbol => {
                    atlas.evaluate_symbol(target_name, local_frame, &mut self.evaluated_parts);
                }
            }
        }
    }

    pub(crate) fn draw_current(&self, batcher: &mut SpriteBatcher, params: &AtlasDrawParams) {
        let Some(anim_idx) = self.current_anim else {
            // If no animation has started yet, try drawing frame 0 of the first atlas.
            if let Some(atlas) = self.atlases.first() {
                match atlas {
                    LoadedAtlasSource::Sparrow {
                        texture,
                        tex_size,
                        atlas,
                    } => {
                        if let Some(frame) = atlas.frames().first() {
                            draw_atlas_frame(
                                batcher, *texture, *tex_size, frame, params, false, false,
                            );
                        }
                    }
                    LoadedAtlasSource::Packer {
                        texture,
                        tex_size,
                        atlas,
                    } => {
                        if let Some(frame) = atlas.frames().first() {
                            draw_atlas_frame(
                                batcher, *texture, *tex_size, frame, params, false, false,
                            );
                        }
                    }
                    LoadedAtlasSource::Animate { .. } => {}
                }
            }
            return;
        };
        let Some(anim) = self.anims.get(anim_idx) else {
            return;
        };

        match &anim.frames {
            AnimFrames::Sheet { atlas_idx, frames } => {
                if frames.is_empty() {
                    return;
                }
                let frame = &frames[self.current_frame.min(frames.len() - 1)];
                let Some(atlas) = self.atlases.get(*atlas_idx) else {
                    return;
                };
                let (texture, tex_size) = match atlas {
                    LoadedAtlasSource::Sparrow {
                        texture, tex_size, ..
                    }
                    | LoadedAtlasSource::Packer {
                        texture, tex_size, ..
                    } => (*texture, *tex_size),
                    LoadedAtlasSource::Animate { .. } => return,
                };
                draw_atlas_frame(
                    batcher,
                    texture,
                    tex_size,
                    frame,
                    params,
                    anim.flip_x,
                    anim.flip_y,
                );
            }
            AnimFrames::Animate { atlas_idx, .. } => {
                let Some(LoadedAtlasSource::Animate {
                    textures,
                    bounds_origin,
                    bounds_size,
                    ..
                }) = self.atlases.get(*atlas_idx)
                else {
                    return;
                };
                draw_animate_parts(
                    batcher,
                    textures,
                    &self.evaluated_parts,
                    *bounds_origin,
                    *bounds_size,
                    params,
                    anim.flip_x,
                    anim.flip_y,
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AtlasDrawParams {
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    pub anim_offsets: [f32; 2],
    pub global_offsets: [f32; 2],
    pub scroll_factor: [f32; 2],
    pub cam_pos: [f32; 2],
    pub cam_zoom: f32,
    pub angle_deg: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub color: [f32; 4],
    pub alpha: f32,
    pub blend: BlendMode,
    pub is_pixel: bool,
}

#[inline]
#[must_use]
pub(crate) fn compute_flipped_trim_offset(
    src_size: [f32; 2],
    trimmed_size: [f32; 2],
    frame_offset: [f32; 2],
    flip_x: bool,
    flip_y: bool,
) -> [f32; 2] {
    let tx = if flip_x {
        src_size[0] + frame_offset[0] - trimmed_size[0]
    } else {
        -frame_offset[0]
    };
    let ty = if flip_y {
        src_size[1] + frame_offset[1] - trimmed_size[1]
    } else {
        -frame_offset[1]
    };
    [tx, ty]
}

pub(crate) fn draw_atlas_frame(
    batcher: &mut SpriteBatcher,
    texture: TextureHandle,
    tex_size: [f32; 2],
    frame: &AtlasFrame,
    params: &AtlasDrawParams,
    anim_flip_x: bool,
    anim_flip_y: bool,
) {
    let src = frame.source_size();
    let trimmed = frame.upright_trimmed_size();
    let flip_x = params.flip_x ^ anim_flip_x;
    let flip_y = params.flip_y ^ anim_flip_y;

    let trim = compute_flipped_trim_offset(
        src,
        trimmed,
        [frame.frame_x, frame.frame_y],
        flip_x,
        flip_y,
    );

    let untrimmed_x =
        params.pos[0] - (params.anim_offsets[0] - params.global_offsets[0]) * params.scale[0];
    let untrimmed_y =
        params.pos[1] - (params.anim_offsets[1] - params.global_offsets[1]) * params.scale[1];

    let world_x = untrimmed_x + trim[0] * params.scale[0];
    let world_y = untrimmed_y + trim[1] * params.scale[1];
    let world_w = trimmed[0] * params.scale[0];
    let world_h = trimmed[1] * params.scale[1];

    let (screen_pos, screen_size) = apply_camera_transform(
        [world_x, world_y],
        [world_w, world_h],
        params.scroll_factor,
        params.cam_pos,
        params.cam_zoom,
    );

    let origin_rel = [
        (src[0] * 0.5 - trim[0]) * params.scale[0] * params.cam_zoom,
        (src[1] * 0.5 - trim[1]) * params.scale[1] * params.cam_zoom,
    ];

    let eff_flip_x = flip_x ^ frame.flip_x;
    let eff_flip_y = flip_y ^ frame.flip_y;
    let mut flags = 0u32;
    if frame.rotated {
        flags |= SPRITE_FLAG_ROTATED_90_CW;
    }
    if eff_flip_x {
        flags |= SPRITE_FLAG_FLIP_X;
    }
    if eff_flip_y {
        flags |= SPRITE_FLAG_FLIP_Y;
    }

    let filter = if params.is_pixel {
        FilterMode::Nearest
    } else {
        FilterMode::Linear
    };
    let inset_px = if matches!(filter, FilterMode::Linear) {
        0.5
    } else {
        0.0
    };
    let tint = [
        params.color[0],
        params.color[1],
        params.color[2],
        params.color[3] * params.alpha,
    ];

    let inst = SpriteInstance::new(screen_pos, screen_size)
        .with_uv_rect(frame.uv_bounds_inset(tex_size[0], tex_size[1], inset_px))
        .with_color(tint)
        .with_rotation(params.angle_deg.to_radians(), origin_rel)
        .with_flags(flags);

    batcher.push(texture, params.blend, filter, inst);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_animate_parts(
    batcher: &mut SpriteBatcher,
    textures: &[(TextureHandle, [f32; 2])],
    parts: &[EvaluatedPart],
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    params: &AtlasDrawParams,
    anim_flip_x: bool,
    anim_flip_y: bool,
) {
    let flip_x = params.flip_x ^ anim_flip_x;
    let flip_y = params.flip_y ^ anim_flip_y;
    let filter = if params.is_pixel {
        FilterMode::Nearest
    } else {
        FilterMode::Linear
    };

    let untrimmed_x =
        params.pos[0] - (params.anim_offsets[0] - params.global_offsets[0]) * params.scale[0];
    let untrimmed_y =
        params.pos[1] - (params.anim_offsets[1] - params.global_offsets[1]) * params.scale[1];

    let shift_bounds = Affine2D::translation(-bounds_origin[0], -bounds_origin[1]);
    let flip_affine = Affine2D {
        a: if flip_x { -1.0 } else { 1.0 },
        b: 0.0,
        c: 0.0,
        d: if flip_y { -1.0 } else { 1.0 },
        tx: if flip_x { bounds_size[0] } else { 0.0 },
        ty: if flip_y { bounds_size[1] } else { 0.0 },
    };
    let scale_affine = Affine2D::scale(params.scale[0], params.scale[1]);

    let rot_affine = if params.angle_deg.abs() > 1e-5 {
        let cx = bounds_size[0] * params.scale[0] * 0.5;
        let cy = bounds_size[1] * params.scale[1] * 0.5;
        Affine2D::translation(cx, cy)
            .mul(Affine2D::rotation(params.angle_deg.to_radians()))
            .mul(Affine2D::translation(-cx, -cy))
    } else {
        Affine2D::IDENTITY
    };

    let world_base = Affine2D::translation(untrimmed_x, untrimmed_y)
        .mul(rot_affine)
        .mul(scale_affine)
        .mul(flip_affine)
        .mul(shift_bounds);

    let cam_offset_x = VIRTUAL_WIDTH * 0.5 * (1.0 - params.cam_zoom)
        - params.cam_pos[0] * params.scroll_factor[0] * params.cam_zoom;
    let cam_offset_y = VIRTUAL_HEIGHT * 0.5 * (1.0 - params.cam_zoom)
        - params.cam_pos[1] * params.scroll_factor[1] * params.cam_zoom;
    let cam_affine = Affine2D::translation(cam_offset_x, cam_offset_y)
        .mul(Affine2D::scale(params.cam_zoom, params.cam_zoom));
    let full_prefix = cam_affine.mul(world_base);

    for part in parts {
        let Some(&(tex, _)) = textures.get(part.spritemap_index) else {
            continue;
        };

        let part_affine = Affine2D {
            a: part.transform.matrix2.x_axis.x,
            b: part.transform.matrix2.x_axis.y,
            c: part.transform.matrix2.y_axis.x,
            d: part.transform.matrix2.y_axis.y,
            tx: part.transform.translation.x,
            ty: part.transform.translation.y,
        };
        let m = full_prefix.mul(part_affine);

        let (upright_w, upright_h) = if part.rotated {
            (part.size[1], part.size[0])
        } else {
            (part.size[0], part.size[1])
        };

        let sx = (m.a * m.a + m.b * m.b).sqrt();
        let sy = (m.c * m.c + m.d * m.d).sqrt();
        if sx <= 1e-6 || sy <= 1e-6 {
            continue;
        }

        let det = m.a * m.d - m.b * m.c;
        let (pos, rot, part_flip_x) = if det < 0.0 {
            (
                [m.tx + m.a * upright_w, m.ty + m.b * upright_w],
                (-m.b).atan2(-m.a),
                true,
            )
        } else {
            ([m.tx, m.ty], m.b.atan2(m.a), false)
        };

        let mut flags = 0u32;
        if part.rotated {
            flags |= SPRITE_FLAG_ROTATED_90_CW;
        }
        if part_flip_x {
            flags |= SPRITE_FLAG_FLIP_X;
        }

        let uv_bounds = [
            part.uv_rect[0],
            part.uv_rect[1],
            part.uv_rect[0] + part.uv_rect[2],
            part.uv_rect[1] + part.uv_rect[3],
        ];
        let tint = [
            part.color[0] * params.color[0],
            part.color[1] * params.color[1],
            part.color[2] * params.color[2],
            part.color[3] * params.color[3] * params.alpha,
        ];

        let inst = SpriteInstance::new(pos, [upright_w * sx, upright_h * sy])
            .with_uv_rect(uv_bounds)
            .with_color(tint)
            .with_rotation(rot, [0.0, 0.0])
            .with_flags(flags);

        batcher.push(tex, params.blend, filter, inst);
    }
}

pub(crate) fn upload_png_path(
    gpu: &mut GpuContext,
    label: Option<&str>,
    path: &Path,
) -> Option<(TextureHandle, [f32; 2])> {
    let bytes = fs::read(path).ok()?;
    let tex = gpu.from_png_bytes(label, &bytes).ok()?;
    let (w, h) = gpu.texture_size(tex)?;
    Some((tex, [w as f32, h as f32]))
}

pub(crate) fn load_sparrow_source(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
) -> Option<LoadedAtlasSource> {
    let png_path = assets.image(key);
    let xml_path = assets.sparrow_xml(key);
    let (texture, tex_size) = upload_png_path(gpu, Some(key), &png_path)?;
    let xml = fs::read_to_string(&xml_path).ok()?;
    let atlas = SparrowAtlas::from_xml(&xml).ok()?;
    Some(LoadedAtlasSource::Sparrow {
        texture,
        tex_size,
        atlas,
    })
}

pub(crate) fn load_packer_source(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
) -> Option<LoadedAtlasSource> {
    let png_path = assets.image(key);
    let txt_path = assets.packer_txt(key);
    let (texture, tex_size) = upload_png_path(gpu, Some(key), &png_path)?;
    let txt = fs::read_to_string(&txt_path).ok()?;
    let atlas = PackerAtlas::from_txt(&txt).ok()?;
    Some(LoadedAtlasSource::Packer {
        texture,
        tex_size,
        atlas,
    })
}

pub(crate) fn load_animate_source(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
    _settings: Option<&TextureAtlasData>,
) -> Option<LoadedAtlasSource> {
    let dir = assets.animate_atlas_dir(key);
    let anim_json_path = dir.join("Animation.json");
    let anim_json = fs::read_to_string(&anim_json_path).ok()?;

    let mut sm_entries: Vec<(u32, std::path::PathBuf)> = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("spritemap") && name.ends_with(".json") {
                let stem = &name["spritemap".len()..name.len() - ".json".len()];
                let num = stem.parse::<u32>().unwrap_or(0);
                sm_entries.push((num, path));
            }
        }
    }
    sm_entries.sort_by_key(|(num, _)| *num);
    if sm_entries.is_empty() {
        return None;
    }

    let mut sm_strings = Vec::with_capacity(sm_entries.len());
    for (_, path) in &sm_entries {
        if let Ok(content) = fs::read_to_string(path) {
            sm_strings.push(content);
        }
    }
    if sm_strings.is_empty() {
        return None;
    }

    let sm_refs: Vec<&str> = sm_strings.iter().map(String::as_str).collect();
    let atlas = AnimateAtlas::from_json_strs(&anim_json, &sm_refs).ok()?;

    let mut textures = Vec::with_capacity(atlas.spritemaps().len());
    for sm in atlas.spritemaps() {
        let img_path = dir.join(&sm.image);
        let uploaded = upload_png_path(gpu, Some(&sm.image), &img_path)?;
        textures.push(uploaded);
    }

    let (bounds_origin, bounds_size) = compute_animate_bounds(&atlas);
    Some(LoadedAtlasSource::Animate {
        textures,
        atlas,
        bounds_origin,
        bounds_size,
    })
}

fn compute_animate_bounds(atlas: &AnimateAtlas) -> ([f32; 2], [f32; 2]) {
    let mut parts = Vec::with_capacity(INITIAL_EVAL_PARTS_CAP);
    if !atlas.evaluate_label("idle", 0, &mut parts)
        && !atlas.evaluate_label("Idle", 0, &mut parts)
    {
        atlas.evaluate_frame(0, &mut parts);
    }
    if parts.is_empty() {
        return ([0.0, 0.0], [1.0, 1.0]);
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for part in &parts {
        let (w, h) = if part.rotated {
            (part.size[1], part.size[0])
        } else {
            (part.size[0], part.size[1])
        };
        let m = Affine2D {
            a: part.transform.matrix2.x_axis.x,
            b: part.transform.matrix2.x_axis.y,
            c: part.transform.matrix2.y_axis.x,
            d: part.transform.matrix2.y_axis.y,
            tx: part.transform.translation.x,
            ty: part.transform.translation.y,
        };
        for &(lx, ly) in &[(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
            let [px, py] = m.transform_point(lx, ly);
            min_x = min_x.min(px);
            min_y = min_y.min(py);
            max_x = max_x.max(px);
            max_y = max_y.max(py);
        }
    }

    if !min_x.is_finite() || !min_y.is_finite() {
        return ([0.0, 0.0], [1.0, 1.0]);
    }

    (
        [min_x, min_y],
        [(max_x - min_x).max(1.0), (max_y - min_y).max(1.0)],
    )
}

pub(crate) fn build_loaded_anim(
    anim: &AnimationData,
    atlases: &[LoadedAtlasSource],
    atlas_idx: usize,
) -> Option<LoadedAnim> {
    let source = atlases.get(atlas_idx)?;
    let frames = match source {
        LoadedAtlasSource::Sparrow { atlas, .. } => {
            let prefix = anim.prefix.as_deref()?;
            let collected = collect_sheet_frames(
                atlas.frames_by_prefix(prefix),
                &anim.frame_indices,
                atlas.frames(),
                true,
            );
            AnimFrames::Sheet {
                atlas_idx,
                frames: collected,
            }
        }
        LoadedAtlasSource::Packer { atlas, .. } => {
            let collected = if let Some(prefix) = anim.prefix.as_deref().filter(|s| !s.is_empty())
            {
                collect_sheet_frames(
                    atlas.frames_by_prefix(prefix),
                    &anim.frame_indices,
                    atlas.frames(),
                    true,
                )
            } else {
                collect_sheet_frames(
                    atlas.frames().iter(),
                    &anim.frame_indices,
                    atlas.frames(),
                    false,
                )
            };
            AnimFrames::Sheet {
                atlas_idx,
                frames: collected,
            }
        }
        LoadedAtlasSource::Animate { atlas, .. } => {
            let prefix = anim.prefix.as_deref()?;
            let mode = if anim.anim_type.eq_ignore_ascii_case("symbol") {
                AnimateAnimMode::Symbol
            } else {
                AnimateAnimMode::FrameLabel
            };
            let total_frames = match mode {
                AnimateAnimMode::FrameLabel => {
                    atlas.label_span(prefix).map_or(0, |s| s.duration)
                }
                AnimateAnimMode::Symbol => atlas.symbol_total_frames(prefix).unwrap_or(0),
            };
            let indices: Vec<u32> = anim.frame_indices.iter().map(|&i| i as u32).collect();
            AnimFrames::Animate {
                atlas_idx,
                mode,
                target_name: prefix.to_owned(),
                indices,
                total_frames,
            }
        }
    };

    let frame_rate = if anim.frame_rate > 0 {
        anim.frame_rate as f32
    } else {
        match source {
            LoadedAtlasSource::Animate { atlas, .. } => atlas.frame_rate(),
            _ => DEFAULT_ANIM_FPS,
        }
    };

    Some(make_loaded_anim_entry(
        anim.name.clone(),
        anim.offsets,
        anim.looped,
        anim.flip_x,
        anim.flip_y,
        frame_rate,
        frames,
    ))
}

pub(crate) fn make_loaded_anim_entry(
    name: String,
    offsets: [f32; 2],
    looped: bool,
    flip_x: bool,
    flip_y: bool,
    frame_rate: f32,
    frames: AnimFrames,
) -> LoadedAnim {
    let is_end_suffix = name.ends_with("-end");
    let is_hold_suffix = name.ends_with("-hold");
    let is_sing = name.starts_with("sing") && !is_end_suffix;
    let is_miss = name.ends_with("miss") || name.contains("miss-");
    let is_idle_or_dance = name.starts_with("idle") || name.starts_with("dance");
    let is_combo_or_drop = name.starts_with("combo") || name.starts_with("drop");

    LoadedAnim {
        name,
        offsets,
        looped,
        flip_x,
        flip_y,
        frame_rate,
        frames,
        hold_anim: None,
        end_anim: None,
        is_sing,
        is_miss,
        is_hold_suffix,
        is_end_suffix,
        is_idle_or_dance,
        is_combo_or_drop,
    }
}

fn collect_sheet_frames<'a>(
    matched: impl Iterator<Item = &'a AtlasFrame>,
    indices: &[usize],
    fallback_all: &[AtlasFrame],
    use_matched_for_indices: bool,
) -> Vec<AtlasFrame> {
    let base: Vec<&AtlasFrame> = matched.collect();
    if indices.is_empty() {
        return base.into_iter().cloned().collect();
    }
    let pool: Vec<&AtlasFrame> = if use_matched_for_indices {
        base
    } else {
        fallback_all.iter().collect()
    };
    let mut out = Vec::with_capacity(indices.len());
    for &idx in indices {
        if let Some(&frame) = pool.get(idx) {
            out.push(frame.clone());
        }
    }
    out
}

pub(crate) fn format_stack_join<'a>(
    buf: &'a mut [u8],
    left: &str,
    right: &str,
) -> Option<&'a str> {
    let total = left.len() + right.len();
    if total > buf.len() {
        return None;
    }
    buf[..left.len()].copy_from_slice(left.as_bytes());
    buf[left.len()..total].copy_from_slice(right.as_bytes());
    std::str::from_utf8(&buf[..total]).ok()
}

struct StackBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> StackBuf<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl std::fmt::Write for StackBuf<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        let avail = self.buf.len().saturating_sub(self.len);
        if bytes.len() > avail {
            return Err(std::fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

#[inline]
#[must_use]
const fn dir_upper(dir: NoteDirection) -> &'static str {
    match dir {
        NoteDirection::Left => "LEFT",
        NoteDirection::Down => "DOWN",
        NoteDirection::Up => "UP",
        NoteDirection::Right => "RIGHT",
    }
}

/// Runtime V-Slice character controller (`BaseCharacter.hx` + Sparrow/Packer/Animate/Multi).
#[derive(Debug)]
pub struct BaseCharacter {
    pub id: String,
    pub name: String,
    pub kind: CharacterKind,
    pub render_type: CharacterRenderType,

    pub position: [f32; 2],
    pub original_position: [f32; 2],
    pub base_scale: f32,
    pub scale: f32,
    pub global_offsets: [f32; 2],
    pub data_camera_offsets: [f32; 2],
    pub stage_camera_offsets: [f32; 2],
    pub camera_focus_point: [f32; 2],
    pub scroll_factor: [f32; 2],
    pub z_index: i32,
    pub alpha: f32,
    pub angle: f32,
    pub is_pixel: bool,
    pub flip_x: bool,
    pub data_flip_x: bool,
    pub dance_every: f32,
    pub data_dance_every: f32,
    pub sing_time: f32,
    pub hold_timer: f32,
    pub is_dead: bool,
    pub should_bop: bool,
    pub should_alternate: bool,
    pub has_danced: bool,
    pub can_play_other_anims: bool,
    pub idle_suffix: String,
    pub starting_animation: String,
    pub health_icon: Option<HealthIconData>,
    pub death_data: Option<DeathData>,
    pub combo_note_counts: Vec<u32>,
    pub drop_note_counts: Vec<u32>,

    ref_unscaled_size: [f32; 2],
    hitbox_size: [f32; 2],
    bank: AnimBank,
    idle_anim: Option<usize>,
    dance_left_anim: Option<usize>,
    dance_right_anim: Option<usize>,
    sing_anims: [Option<usize>; KEY_COUNT],
    sing_miss_anims: [Option<usize>; KEY_COUNT],
    last_dance_step: Option<i32>,
}

impl BaseCharacter {
    /// Loads `data/characters/<id>.json` and builds a [`BaseCharacter`] via `assets` & `gpu`.
    pub fn load_by_id(
        id: &str,
        kind: CharacterKind,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Option<Self> {
        let key = format!("characters/{id}");
        let path = assets.data_json(&key);
        let mut raw = fs::read_to_string(&path).ok()?;
        let trimmed = raw.trim_end();
        if let Some(pos) = trimmed.rfind('}') {
            raw.truncate(pos + 1);
        }
        let data: CharacterData = serde_json::from_str(&raw).ok()?;
        Some(Self::load_from_assets(id, kind, data, assets, gpu))
    }

    /// Builds a [`BaseCharacter`] from parsed [`CharacterData`], loading all primary
    /// and secondary (Multi-Sparrow / Multi-Animate) atlases into `gpu`.
    pub fn load_from_assets(
        id: &str,
        kind: CharacterKind,
        data: CharacterData,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Self {
        let mut bank = AnimBank::new();
        let mut path_to_atlas: HashMap<String, usize> = HashMap::new();

        // Load primary atlas according to `data.render_type`.
        let primary = match data.render_type {
            CharacterRenderType::Packer => load_packer_source(assets, gpu, &data.asset_path),
            CharacterRenderType::AnimateAtlas | CharacterRenderType::MultiAnimateAtlas => {
                load_animate_source(
                    assets,
                    gpu,
                    &data.asset_path,
                    data.atlas_settings.as_ref(),
                )
            }
            CharacterRenderType::Sparrow
            | CharacterRenderType::MultiSparrow
            | CharacterRenderType::Custom => load_sparrow_source(assets, gpu, &data.asset_path),
        };

        let primary_idx = if let Some(src) = primary {
            let idx = bank.atlases.len();
            bank.atlases.push(src);
            path_to_atlas.insert(data.asset_path.clone(), idx);
            Some(idx)
        } else {
            None
        };

        // Load secondary atlases for MultiSparrow / MultiAnimateAtlas (or per-anim assetPath).
        for anim in &data.animations {
            let Some(sub_path) = anim.asset_path.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            if path_to_atlas.contains_key(sub_path) {
                continue;
            }
            let use_animate = anim.render_type.eq_ignore_ascii_case("animateatlas")
                || (data.render_type == CharacterRenderType::MultiAnimateAtlas
                    && !anim.render_type.eq_ignore_ascii_case("sparrow")
                    && !anim.render_type.eq_ignore_ascii_case("packer"));

            let sub_src = if use_animate {
                load_animate_source(
                    assets,
                    gpu,
                    sub_path,
                    anim.atlas_settings
                        .as_ref()
                        .or(data.atlas_settings.as_ref()),
                )
            } else if anim.render_type.eq_ignore_ascii_case("packer") {
                load_packer_source(assets, gpu, sub_path)
            } else {
                load_sparrow_source(assets, gpu, sub_path)
            };

            if let Some(src) = sub_src {
                let idx = bank.atlases.len();
                bank.atlases.push(src);
                path_to_atlas.insert(sub_path.to_owned(), idx);
            }
        }

        // Build animations, falling back across loaded atlases for MultiSparrow sheets.
        for anim in &data.animations {
            if let Some(sub_path) = anim.asset_path.as_deref().filter(|s| !s.is_empty()) {
                if let Some(&idx) = path_to_atlas.get(sub_path) {
                    if let Some(loaded) = build_loaded_anim(anim, &bank.atlases, idx) {
                        bank.anims.push(loaded);
                    }
                }
                continue;
            }

            if let Some(p_idx) = primary_idx {
                if let Some(mut loaded) = build_loaded_anim(anim, &bank.atlases, p_idx) {
                    if loaded.frames.frame_count() == 0 && bank.atlases.len() > 1 {
                        for alt_idx in 0..bank.atlases.len() {
                            if alt_idx == p_idx {
                                continue;
                            }
                            if let Some(alt_loaded) =
                                build_loaded_anim(anim, &bank.atlases, alt_idx)
                            {
                                if alt_loaded.frames.frame_count() > 0 {
                                    loaded = alt_loaded;
                                    break;
                                }
                            }
                        }
                    }
                    bank.anims.push(loaded);
                }
            }
        }

        Self::finish_init(id, kind, data, bank)
    }

    /// Creates a headless [`BaseCharacter`] from [`CharacterData`] without GPU textures
    /// (for tests and simulation without a window).
    #[must_use]
    pub fn from_data_headless(id: &str, kind: CharacterKind, data: CharacterData) -> Self {
        let mut bank = AnimBank::new();
        for anim in &data.animations {
            let frames = AnimFrames::Sheet {
                atlas_idx: 0,
                frames: vec![AtlasFrame {
                    name: anim.prefix.clone().unwrap_or_else(|| anim.name.clone()),
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 150,
                    frame_x: 0.0,
                    frame_y: 0.0,
                    frame_width: 100.0,
                    frame_height: 150.0,
                    rotated: false,
                    flip_x: false,
                    flip_y: false,
                }],
            };
            bank.anims.push(make_loaded_anim_entry(
                anim.name.clone(),
                anim.offsets,
                anim.looped,
                anim.flip_x,
                anim.flip_y,
                anim.frame_rate.max(1) as f32,
                frames,
            ));
        }
        Self::finish_init(id, kind, data, bank)
    }

    /// Creates a synthetic [`BaseCharacter`] with explicit hitbox size and animation list
    /// for unit testing positioning, offsets, dancing, and sing timers.
    #[must_use]
    pub fn new_synthetic(
        id: &str,
        kind: CharacterKind,
        unscaled_frame_size: [f32; 2],
        data: CharacterData,
    ) -> Self {
        let mut ch = Self::from_data_headless(id, kind, data);
        ch.ref_unscaled_size = unscaled_frame_size;
        ch.hitbox_size = [
            unscaled_frame_size[0] * ch.scale,
            unscaled_frame_size[1] * ch.scale,
        ];
        ch.reset_camera_focus_point();
        ch
    }

    fn finish_init(
        id: &str,
        kind: CharacterKind,
        data: CharacterData,
        mut bank: AnimBank,
    ) -> Self {
        bank.finalize_links();

        let idle_anim = bank.anim_index.get("idle").copied();
        let dance_left_anim = bank.anim_index.get("danceLeft").copied();
        let dance_right_anim = bank.anim_index.get("danceRight").copied();
        let should_alternate = dance_left_anim.is_some();

        let mut sing_anims = [None; KEY_COUNT];
        let mut sing_miss_anims = [None; KEY_COUNT];
        for dir in NoteDirection::ALL {
            let d = dir_upper(dir);
            let mut s_buf = [0u8; 32];
            if let Some(key) = format_stack_join(&mut s_buf, "sing", d) {
                sing_anims[dir.as_index()] = bank.anim_index.get(key).copied();
            }
            let mut m_buf = [0u8; 32];
            let mut sb = StackBuf::new(&mut m_buf);
            if write!(sb, "sing{d}miss").is_ok() {
                sing_miss_anims[dir.as_index()] = bank.anim_index.get(sb.as_str()).copied();
            }
        }

        let mut combo_note_counts = Vec::new();
        let mut drop_note_counts = Vec::new();
        for anim in &bank.anims {
            if let Some(rest) = anim.name.strip_prefix("combo") {
                if let Ok(n) = rest.parse::<u32>() {
                    combo_note_counts.push(n);
                }
            } else if let Some(rest) = anim.name.strip_prefix("drop") {
                if let Ok(n) = rest.parse::<u32>() {
                    drop_note_counts.push(n);
                }
            }
        }
        combo_note_counts.sort_unstable();
        drop_note_counts.sort_unstable();

        // Pick reference frame dimensions from idle/danceLeft/starting animation.
        let ref_anim_idx = if should_alternate {
            dance_left_anim
        } else {
            idle_anim
        }
        .or_else(|| bank.anim_index.get(&data.starting_animation).copied())
        .or(if bank.anims.is_empty() { None } else { Some(0) });

        let ref_unscaled_size = ref_anim_idx
            .and_then(|idx| bank.anims[idx].frames.first_source_size(&bank.atlases))
            .or_else(|| {
                bank.atlases
                    .first()
                    .and_then(LoadedAtlasSource::fallback_hitbox_size)
            })
            .unwrap_or([100.0, 150.0]);

        let base_scale = data.scale;
        let hitbox_size = [
            ref_unscaled_size[0] * base_scale,
            ref_unscaled_size[1] * base_scale,
        ];

        let flip_x = match kind {
            CharacterKind::Bf => !data.flip_x,
            CharacterKind::Dad | CharacterKind::Gf | CharacterKind::Other => data.flip_x,
        };

        let mut ch = Self {
            id: id.to_owned(),
            name: data.name,
            kind,
            render_type: data.render_type,
            position: [0.0, 0.0],
            original_position: [0.0, 0.0],
            base_scale,
            scale: base_scale,
            global_offsets: data.offsets,
            data_camera_offsets: data.camera_offsets,
            stage_camera_offsets: [0.0, 0.0],
            camera_focus_point: [0.0, 0.0],
            scroll_factor: [1.0, 1.0],
            z_index: 0,
            alpha: 1.0,
            angle: 0.0,
            is_pixel: data.is_pixel,
            flip_x,
            data_flip_x: data.flip_x,
            dance_every: data.dance_every,
            data_dance_every: data.dance_every,
            sing_time: data.sing_time,
            hold_timer: 0.0,
            is_dead: false,
            should_bop: false,
            should_alternate,
            has_danced: false,
            can_play_other_anims: true,
            idle_suffix: String::new(),
            starting_animation: data.starting_animation,
            health_icon: data.health_icon,
            death_data: data.death,
            combo_note_counts,
            drop_note_counts,
            ref_unscaled_size,
            hitbox_size,
            bank,
            idle_anim,
            dance_left_anim,
            dance_right_anim,
            sing_anims,
            sing_miss_anims,
            last_dance_step: None,
        };

        ch.dance(true);
        if ch.bank.current_anim.is_none() && !ch.starting_animation.is_empty() {
            let start_name = ch.starting_animation.clone();
            ch.play_animation(&start_name, true, false);
        }
        ch.reset_camera_focus_point();
        ch
    }

    /// Offset from top-left `position` to ground origin (horizontal center, vertical bottom).
    #[inline]
    #[must_use]
    pub fn character_origin(&self) -> [f32; 2] {
        [self.hitbox_size[0] * 0.5, self.hitbox_size[1]]
    }

    /// Scaled hitbox dimensions `[width, height]`.
    #[inline]
    #[must_use]
    pub fn hitbox_size(&self) -> [f32; 2] {
        self.hitbox_size
    }

    /// World position of the character's feet (`position + character_origin`).
    #[inline]
    #[must_use]
    pub fn feet_position(&self) -> [f32; 2] {
        let origin = self.character_origin();
        [self.position[0] + origin[0], self.position[1] + origin[1]]
    }

    /// Places the character so its feet rest at `feet` (applying `global_offsets`).
    pub fn set_feet_position(&mut self, feet: [f32; 2]) {
        let origin = self.character_origin();
        self.position = [
            feet[0] - origin[0] + self.global_offsets[0],
            feet[1] - origin[1] + self.global_offsets[1],
        ];
        self.original_position = self.position;
        self.reset_camera_focus_point();
    }

    /// Updates `scale` while keeping feet anchored (`BaseCharacter.setScale`).
    pub fn set_scale(&mut self, scale: f32) {
        let feet = self.feet_position();
        self.scale = scale;
        self.hitbox_size = [
            self.ref_unscaled_size[0] * scale,
            self.ref_unscaled_size[1] * scale,
        ];
        let origin = self.character_origin();
        self.position = [
            feet[0] - origin[0] + self.global_offsets[0],
            feet[1] - origin[1] + self.global_offsets[1],
        ];
    }

    /// Positions and configures this character from stage placement data (`Stage.addCharacter`).
    pub fn place_on_stage(&mut self, placement: &StageDataCharacter) {
        self.flip_x = match self.kind {
            CharacterKind::Bf => !self.data_flip_x,
            CharacterKind::Dad | CharacterKind::Gf | CharacterKind::Other => self.data_flip_x,
        };
        self.reset_character(false);

        self.z_index = placement.z_index;
        let origin = self.character_origin();
        self.position = [
            placement.position[0] - origin[0],
            placement.position[1] - origin[1],
        ];
        let final_scale = self.base_scale * placement.scale;
        self.set_scale(final_scale);
        self.original_position = self.position;

        self.stage_camera_offsets = placement.camera_offsets;
        self.scroll_factor = placement.scroll;
        self.alpha = placement.alpha;
        self.angle = placement.angle;
        self.reset_camera_focus_point();
    }

    /// Resets position, dance cadence, idle pose, and optionally `camera_focus_point`.
    pub fn reset_character(&mut self, reset_camera: bool) {
        self.position = self.original_position;
        self.dance_every = self.data_dance_every;
        self.hold_timer = 0.0;
        self.has_danced = false;
        self.can_play_other_anims = true;
        self.last_dance_step = None;
        self.dance(true);
        if reset_camera {
            self.reset_camera_focus_point();
        }
    }

    /// Recomputes `camera_focus_point` from `original_position`, hitbox center, and offsets.
    pub fn reset_camera_focus_point(&mut self) {
        let cx = self.original_position[0] + self.hitbox_size[0] * 0.5;
        let cy = self.original_position[1] + self.hitbox_size[1] * 0.5;
        self.camera_focus_point = [
            cx + self.data_camera_offsets[0] + self.stage_camera_offsets[0],
            cy + self.data_camera_offsets[1] + self.stage_camera_offsets[1],
        ];
    }

    /// Returns the world point the game camera focuses on for this character.
    #[inline]
    #[must_use]
    pub fn get_camera_focus_point(&self) -> [f32; 2] {
        self.camera_focus_point
    }

    /// Returns the active animation name, or `""` if none.
    #[inline]
    #[must_use]
    pub fn current_animation(&self) -> &str {
        self.bank.current_name()
    }

    /// Returns the active animation's `[x, y]` offset.
    #[inline]
    #[must_use]
    pub fn current_anim_offsets(&self) -> [f32; 2] {
        self.bank.current_offsets()
    }

    /// Returns `true` if the active animation has reached its final frame.
    #[inline]
    #[must_use]
    pub fn is_animation_finished(&self) -> bool {
        self.bank.finished
    }

    /// Returns `true` when currently playing a `sing*` (non-`-end`) animation.
    #[inline]
    #[must_use]
    pub fn is_singing(&self) -> bool {
        self.bank
            .current_anim_meta()
            .is_some_and(|anim| anim.is_sing)
    }

    /// Returns `true` if `name` is present in this character's animation table.
    #[inline]
    #[must_use]
    pub fn has_animation(&self, name: &str) -> bool {
        self.bank.anim_index.contains_key(name)
    }

    /// Computes the draw-space world top-left position for the current frame after
    /// `anim_offsets`, `global_offsets`, and `scale` (`Bopper.getScreenPosition`).
    #[inline]
    #[must_use]
    pub fn effective_draw_origin(&self) -> [f32; 2] {
        let anim_off = self.current_anim_offsets();
        [
            self.position[0] - (anim_off[0] - self.global_offsets[0]) * self.scale,
            self.position[1] - (anim_off[1] - self.global_offsets[1]) * self.scale,
        ]
    }

    /// Sets the idle suffix (e.g. `"-alt"`) and immediately re-dances (`Bopper.set_idleSuffix`).
    pub fn set_idle_suffix(&mut self, suffix: &str) {
        self.idle_suffix.clear();
        self.idle_suffix.push_str(suffix);
        self.dance(false);
    }

    /// Plays an animation by name with `Bopper.correctAnimationName` `-suffix` fallback
    /// and zero heap allocations.
    pub fn play_animation(&mut self, name: &str, restart: bool, ignore_other: bool) {
        if !self.can_play_other_anims {
            let same_restart = self.current_animation() == name && restart;
            if !same_restart && !name.starts_with("sing") {
                return;
            }
        }
        let Some(idx) = self.bank.resolve_with_fallback(name) else {
            return;
        };
        self.play_anim_by_index(idx, restart, ignore_other);
    }

    fn play_anim_by_index(&mut self, idx: usize, restart: bool, ignore_other: bool) {
        self.bank.play_index(idx, restart);
        if ignore_other {
            self.can_play_other_anims = false;
        }
    }

    /// Plays `sing{DIR}[miss][-{suffix}]` with zero heap allocations.
    pub fn play_sing_animation(&mut self, dir: NoteDirection, is_miss: bool, suffix: &str) {
        if suffix.is_empty() {
            let pre = if is_miss {
                self.sing_miss_anims[dir.as_index()]
            } else {
                self.sing_anims[dir.as_index()]
            };
            if let Some(idx) = pre {
                // `sing` is in `ignoreExclusionPref` so it overrides `canPlayOtherAnims`.
                self.play_anim_by_index(idx, true, false);
                self.hold_timer = 0.0;
                return;
            }
        }

        let mut buf = [0u8; 96];
        let mut sb = StackBuf::new(&mut buf);
        let d = dir_upper(dir);
        let miss_part = if is_miss { "miss" } else { "" };
        let res = if suffix.is_empty() {
            write!(sb, "sing{d}{miss_part}")
        } else {
            write!(sb, "sing{d}{miss_part}-{suffix}")
        };
        if res.is_ok() {
            let target = sb.as_str();
            if let Some(idx) = self.bank.resolve_with_fallback(target) {
                self.play_anim_by_index(idx, true, false);
                self.hold_timer = 0.0;
            }
        }
    }

    /// Plays `combo{count}` if registered for GF-style characters.
    pub fn play_combo_animation(&mut self, combo_count: u32) {
        let mut buf = [0u8; 32];
        let mut sb = StackBuf::new(&mut buf);
        if write!(sb, "combo{combo_count}").is_ok() {
            if let Some(&idx) = self.bank.anim_index.get(sb.as_str()) {
                self.play_anim_by_index(idx, true, true);
            }
        }
    }

    /// Plays the highest matching `drop{threshold}` animation when dropping a combo >= threshold.
    pub fn play_combo_drop_animation(&mut self, combo_count: u32) {
        let mut chosen: Option<u32> = None;
        for &count in &self.drop_note_counts {
            if combo_count >= count {
                chosen = Some(count);
            }
        }
        if let Some(threshold) = chosen {
            let mut buf = [0u8; 32];
            let mut sb = StackBuf::new(&mut buf);
            if write!(sb, "drop{threshold}").is_ok() {
                if let Some(&idx) = self.bank.anim_index.get(sb.as_str()) {
                    self.play_anim_by_index(idx, true, true);
                }
            }
        }
    }

    /// Plays `danceLeft`/`danceRight` (when alternating) or `idle` (`BaseCharacter.dance`).
    pub fn dance(&mut self, force: bool) {
        if self.is_dead {
            return;
        }
        if !force {
            if self.is_singing() {
                return;
            }
            if let Some(meta) = self.bank.current_anim_meta() {
                if !meta.is_idle_or_dance && !self.bank.finished {
                    return;
                }
            }
        }

        if self.should_alternate {
            let play_right = self.has_danced;
            let played = if self.idle_suffix.is_empty() {
                let target = if play_right {
                    self.dance_right_anim
                } else {
                    self.dance_left_anim
                };
                if let Some(idx) = target {
                    self.play_anim_by_index(idx, force, false);
                    true
                } else {
                    false
                }
            } else {
                let mut buf = [0u8; 96];
                let base = if play_right { "danceRight" } else { "danceLeft" };
                if let Some(name) = format_stack_join(&mut buf, base, &self.idle_suffix) {
                    if let Some(idx) = self.bank.resolve_with_fallback(name) {
                        self.play_anim_by_index(idx, force, false);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if played {
                self.has_danced = !self.has_danced;
            }
        } else if self.idle_suffix.is_empty() {
            if let Some(idx) = self.idle_anim {
                self.play_anim_by_index(idx, force, false);
            }
        } else {
            let mut buf = [0u8; 96];
            if let Some(name) = format_stack_join(&mut buf, "idle", &self.idle_suffix) {
                if let Some(idx) = self.bank.resolve_with_fallback(name) {
                    self.play_anim_by_index(idx, force, false);
                }
            }
        }
    }

    /// Resets `hold_timer` (e.g. when player presses a note key).
    #[inline]
    pub fn reset_hold_timer(&mut self) {
        self.hold_timer = 0.0;
    }

    /// Beat callback matching `BaseCharacter` / `Bopper` dance cadence.
    pub fn on_beat_hit(&mut self, beat: i32, is_holding_note: bool) {
        if self.dance_every <= 0.0 || beat < 0 {
            return;
        }
        let step = beat * (STEPS_PER_BEAT as i32);
        let steps_per_dance = (self.dance_every * STEPS_PER_BEAT as f32).round() as i32;
        if steps_per_dance <= 0 {
            return;
        }
        let should_dance = if self.dance_every < 1.0 {
            true
        } else {
            step % steps_per_dance == 0
        };
        if should_dance && self.last_dance_step != Some(step) {
            self.last_dance_step = Some(step);
            if self.kind == CharacterKind::Bf && is_holding_note && self.is_singing() {
                return;
            }
            self.dance(self.should_bop);
        }
    }

    /// Step callback supporting fractional `dance_every` down to `0.25` (`Bopper.onStepHit`).
    pub fn on_step_hit(&mut self, step: i32, is_holding_note: bool) {
        if self.dance_every <= 0.0 || step < 0 {
            return;
        }
        let steps_per_dance = (self.dance_every * STEPS_PER_BEAT as f32).round() as i32;
        if steps_per_dance <= 0 {
            return;
        }
        if step % steps_per_dance == 0 && self.last_dance_step != Some(step) {
            self.last_dance_step = Some(step);
            if self.kind == CharacterKind::Bf && is_holding_note && self.is_singing() {
                return;
            }
            self.dance(self.should_bop);
        }
    }

    /// Zero-allocation per-frame character update (`BaseCharacter.onUpdate`).
    ///
    /// - Advances current animation frames.
    /// - Transitions `anim` -> `anim-hold` when finished (if available).
    /// - Reverts `sing*` animations to `sing*-end` or `dance(true)` once
    ///   `hold_timer > sing_time * step_length_s` (doubled on miss) when not holding.
    pub fn update(&mut self, elapsed_s: f32, beat_length_ms: f64, is_holding: bool) {
        let just_finished = self.bank.update(elapsed_s);
        if just_finished {
            if !self.can_play_other_anims {
                self.can_play_other_anims = true;
            }
            let should_redance = self.bank.current_anim_meta().is_some_and(|meta| {
                (meta.is_end_suffix && !meta.is_idle_or_dance) || meta.is_combo_or_drop
            });
            if should_redance {
                self.dance(true);
            }
        }

        if self.is_dead {
            return;
        }

        // Transition finished non-`-hold` animations into their `-hold` loop if defined.
        if self.bank.finished {
            let hold_idx = self
                .bank
                .current_anim_meta()
                .filter(|meta| !meta.is_hold_suffix)
                .and_then(|meta| meta.hold_anim);
            if let Some(idx) = hold_idx {
                self.play_anim_by_index(idx, true, false);
            }
        }

        // Sing hold timer revert (`BaseCharacter.hx` lines 426-469).
        let sing_info = self
            .bank
            .current_anim_meta()
            .filter(|meta| meta.is_sing)
            .map(|meta| (meta.is_miss, meta.end_anim));

        if let Some((is_miss, end_anim)) = sing_info {
            self.hold_timer += elapsed_s.max(0.0);
            let step_ms = beat_length_ms.max(1.0) / f64::from(STEPS_PER_BEAT);
            let mut sing_time_sec = self.sing_time * (step_ms / MS_PER_SEC) as f32;
            if is_miss {
                sing_time_sec *= 2.0;
            }
            let should_stop = match self.kind {
                CharacterKind::Bf => !is_holding,
                CharacterKind::Dad | CharacterKind::Gf | CharacterKind::Other => true,
            };
            if self.hold_timer > sing_time_sec && should_stop {
                self.hold_timer = 0.0;
                if let Some(end_idx) = end_anim {
                    self.play_anim_by_index(end_idx, false, false);
                } else {
                    self.dance(true);
                }
            }
        } else {
            self.hold_timer = 0.0;
        }
    }

    /// Zero-allocation character draw into `batcher` using camera scroll & zoom.
    pub fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if self.alpha <= 0.0 {
            return;
        }
        let params = AtlasDrawParams {
            pos: self.position,
            scale: [self.scale, self.scale],
            anim_offsets: self.current_anim_offsets(),
            global_offsets: self.global_offsets,
            scroll_factor: self.scroll_factor,
            cam_pos,
            cam_zoom,
            angle_deg: self.angle,
            flip_x: self.flip_x,
            flip_y: false,
            color: [1.0, 1.0, 1.0, 1.0],
            alpha: self.alpha,
            blend: BlendMode::Normal,
            is_pixel: self.is_pixel,
        };
        self.bank.draw_current(batcher, &params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn make_anim(name: &str, offsets: [f32; 2], looped: bool) -> AnimationData {
        AnimationData {
            name: name.to_owned(),
            prefix: Some(name.to_owned()),
            asset_path: None,
            offsets,
            looped,
            flip_x: false,
            flip_y: false,
            frame_rate: 24,
            frame_indices: Vec::new(),
            anim_type: "framelabel".to_owned(),
            render_type: String::new(),
            atlas_settings: None,
        }
    }

    fn test_char_data(flip_x: bool, sing_time: f32, anims: Vec<AnimationData>) -> CharacterData {
        CharacterData {
            version: "1.0.2".to_owned(),
            name: "Test".to_owned(),
            render_type: CharacterRenderType::Sparrow,
            asset_path: "characters/test".to_owned(),
            scale: 1.0,
            health_icon: None,
            death: None,
            offsets: [0.0, 0.0],
            camera_offsets: [0.0, 0.0],
            is_pixel: false,
            dance_every: 1.0,
            sing_time,
            animations: anims,
            starting_animation: "idle".to_owned(),
            flip_x,
            apply_stage_matrix: false,
            atlas_settings: None,
        }
    }

    #[test]
    fn sing_timer_reverts_when_not_holding_and_respects_hold() {
        let data = test_char_data(
            false,
            4.0, // 4 steps = 1 beat = 0.5s at 120 BPM (500ms beat)
            vec![
                make_anim("idle", [0.0, 0.0], false),
                make_anim("singLEFT", [10.0, -5.0], false),
                make_anim("singRIGHTmiss", [-5.0, 0.0], false),
            ],
        );
        let mut bf = BaseCharacter::new_synthetic("bf", CharacterKind::Bf, [200.0, 300.0], data);
        assert_eq!(bf.current_animation(), "idle");

        bf.play_sing_animation(NoteDirection::Left, false, "");
        assert_eq!(bf.current_animation(), "singLEFT");
        assert!(bf.is_singing());

        // Hold note past 0.5s: BF stays singing while `is_holding == true`.
        bf.update(0.6, 500.0, true);
        assert_eq!(bf.current_animation(), "singLEFT");

        // Release note: BF reverts to idle on the next update past sing_time.
        bf.update(0.01, 500.0, false);
        assert_eq!(bf.current_animation(), "idle");
        assert!(!bf.is_singing());

        // Miss animation doubles sing_time (0.5s * 2 = 1.0s).
        bf.play_sing_animation(NoteDirection::Right, true, "");
        assert_eq!(bf.current_animation(), "singRIGHTmiss");
        bf.update(0.7, 500.0, false);
        assert_eq!(bf.current_animation(), "singRIGHTmiss");
        bf.update(0.4, 500.0, false);
        assert_eq!(bf.current_animation(), "idle");
    }

    #[test]
    fn dance_left_right_alternates_vs_idle() {
        let gf_data = test_char_data(
            false,
            8.0,
            vec![
                make_anim("danceLeft", [0.0, 0.0], false),
                make_anim("danceRight", [0.0, 0.0], false),
            ],
        );
        let mut gf = BaseCharacter::new_synthetic("gf", CharacterKind::Gf, [300.0, 400.0], gf_data);
        // `finish_init` runs initial `dance(true)` -> `danceLeft`, `has_danced` becomes `true`.
        assert_eq!(gf.current_animation(), "danceLeft");

        gf.on_beat_hit(1, false);
        assert_eq!(gf.current_animation(), "danceRight");
        gf.on_beat_hit(2, false);
        assert_eq!(gf.current_animation(), "danceLeft");

        let dad_data = test_char_data(false, 8.0, vec![make_anim("idle", [0.0, 0.0], false)]);
        let mut dad =
            BaseCharacter::new_synthetic("dad", CharacterKind::Dad, [250.0, 450.0], dad_data);
        assert_eq!(dad.current_animation(), "idle");
        dad.on_beat_hit(1, false);
        assert_eq!(dad.current_animation(), "idle");
    }

    #[test]
    fn feet_position_and_flip_x_offset_math() {
        let mut data = test_char_data(
            false,
            8.0,
            vec![
                make_anim("idle", [0.0, 0.0], false),
                make_anim("singLEFT", [20.0, -10.0], false),
            ],
        );
        data.offsets = [12.0, -8.0];
        data.scale = 1.0;

        let mut bf = BaseCharacter::new_synthetic("bf", CharacterKind::Bf, [200.0, 300.0], data);
        // Bf flips `!data.flip_x` -> `true`.
        assert!(bf.flip_x);

        let placement = StageDataCharacter {
            z_index: 300,
            position: [1000.0, 900.0],
            scale: 2.0,
            camera_offsets: [-100.0, -100.0],
            scroll: [1.0, 1.0],
            alpha: 1.0,
            angle: 0.0,
        };
        bf.place_on_stage(&placement);

        // Hitbox is [200*2, 300*2] = [400, 600], origin = [200, 600].
        // Position = [1000 - 200 + 12, 900 - 600 - 8] = [812, 292].
        assert!(approx_eq(bf.position[0], 812.0));
        assert!(approx_eq(bf.position[1], 292.0));
        let feet = bf.feet_position();
        assert!(approx_eq(feet[0], 1012.0));
        assert!(approx_eq(feet[1], 892.0));

        // Camera focus = [812 + 200 - 100, 292 + 300 - 100] = [912, 492].
        let focus = bf.get_camera_focus_point();
        assert!(approx_eq(focus[0], 912.0));
        assert!(approx_eq(focus[1], 492.0));

        // Play singLEFT (offsets [20, -10]) at scale 2.0:
        // effective_draw_origin = pos - (anim_off - global_off) * scale
        // x = 812 - (20 - 12) * 2 = 796; y = 292 - (-10 - (-8)) * 2 = 296.
        bf.play_sing_animation(NoteDirection::Left, false, "");
        let draw_orig = bf.effective_draw_origin();
        assert!(approx_eq(draw_orig[0], 796.0));
        assert!(approx_eq(draw_orig[1], 296.0));

        // Flipped trim offset math inside [200, 300] for trimmed [120, 260] at (-10, -20).
        let unflipped = compute_flipped_trim_offset(
            [200.0, 300.0],
            [120.0, 260.0],
            [-10.0, -20.0],
            false,
            false,
        );
        assert!(approx_eq(unflipped[0], 10.0));
        assert!(approx_eq(unflipped[1], 20.0));
        let flipped_x = compute_flipped_trim_offset(
            [200.0, 300.0],
            [120.0, 260.0],
            [-10.0, -20.0],
            true,
            false,
        );
        // Remaining right margin was 200 - 10 - 120 = 70 -> flipped X = 70.
        assert!(approx_eq(flipped_x[0], 70.0));
        assert!(approx_eq(flipped_x[1], 20.0));
    }

    #[test]
    fn hold_and_end_suffix_transitions() {
        let mut idle_hold = make_anim("singUP-hold", [0.0, 0.0], true);
        idle_hold.frame_rate = 24;
        let data = test_char_data(
            false,
            2.0, // 2 steps = 0.25s at 120 BPM
            vec![
                make_anim("idle", [0.0, 0.0], false),
                make_anim("singUP", [0.0, 0.0], false),
                idle_hold,
                make_anim("singUP-end", [0.0, 0.0], false),
            ],
        );
        let mut ch = BaseCharacter::new_synthetic("dad", CharacterKind::Dad, [200.0, 300.0], data);
        ch.play_sing_animation(NoteDirection::Up, false, "");
        assert_eq!(ch.current_animation(), "singUP");

        // After 1/24s (~0.05s), 1-frame singUP finishes and switches to singUP-hold.
        ch.update(0.05, 500.0, true);
        assert_eq!(ch.current_animation(), "singUP-hold");

        // After > 0.25s total, Dad reverts via singUP-end then back to idle on finish.
        ch.update(0.25, 500.0, true);
        assert_eq!(ch.current_animation(), "singUP-end");
        ch.update(0.05, 500.0, false);
        assert_eq!(ch.current_animation(), "idle");
    }
}
