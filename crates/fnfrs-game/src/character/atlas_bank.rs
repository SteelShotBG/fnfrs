use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::character::AnimationData;
use fnfrs_core::schema::stage::TextureAtlasData;
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BlendMode, EvaluatedPart, FilterMode, GpuContext, PackerAtlas,
    SparrowAtlas, SpriteBatcher, SpriteInstance, TextureHandle, SPRITE_FLAG_AFFINE,
    SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y, SPRITE_FLAG_ROTATED_90_CCW,
};

use crate::input::NoteDirection;
pub(super) const DEFAULT_ANIM_FPS: f32 = 24.0;
const INITIAL_EVAL_PARTS_CAP: usize = 64;
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
    #[allow(dead_code)]
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
        apply_stage_matrix: bool,
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
        bounds_origin: [f32; 2],
        bounds_size: [f32; 2],
        stage_matrix: Option<Affine2D>,
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
    pub(crate) fn first_source_size(&self, _atlases: &[LoadedAtlasSource]) -> Option<[f32; 2]> {
        match self {
            Self::Sheet { frames, .. } => frames.first().map(AtlasFrame::source_size),
            Self::Animate { bounds_size, .. } => Some(*bounds_size),
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
            // Match Haxe `Map.set`: later entries with the same animation name override earlier ones
            // (e.g. `bf-holding-gf.json` defines a stale `singUP` before the real `BF NOTE UP0`).
            self.anim_index.insert(anim.name.clone(), idx);
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
            ..
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
            AnimFrames::Animate {
                atlas_idx,
                bounds_origin,
                bounds_size,
                stage_matrix,
                ..
            } => {
                let Some(LoadedAtlasSource::Animate { textures, .. }) = self.atlases.get(*atlas_idx)
                else {
                    return;
                };
                draw_animate_parts(
                    batcher,
                    textures,
                    &self.evaluated_parts,
                    *bounds_origin,
                    *bounds_size,
                    *stage_matrix,
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
    pub adjust_color: Option<[f32; 4]>,
    pub wiggle: Option<[f32; 4]>,
    #[allow(dead_code)]
    pub ref_unscaled_size: Option<[f32; 2]>,
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

    let untrimmed_x = params.pos[0]
        - (params.anim_offsets[0] - params.global_offsets[0]) * params.scale[0];
    let untrimmed_y = params.pos[1]
        - (params.anim_offsets[1] - params.global_offsets[1]) * params.scale[1];

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
        flags |= SPRITE_FLAG_ROTATED_90_CCW;
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

    let mut inst = SpriteInstance::new(screen_pos, screen_size)
        .with_uv_rect(frame.uv_bounds_inset(tex_size[0], tex_size[1], inset_px))
        .with_color(tint)
        .with_rotation(params.angle_deg.to_radians(), origin_rel)
        .with_flags(flags);
    if let Some([h, s, b, c]) = params.adjust_color {
        inst = inst.with_adjust_color(h, s, b, c);
    } else if let Some([t, f, sp, amp]) = params.wiggle {
        inst = inst.with_wiggle(t, f, sp, amp);
    }

    batcher.push(texture, params.blend, filter, inst);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_animate_parts(
    batcher: &mut SpriteBatcher,
    textures: &[(TextureHandle, [f32; 2])],
    parts: &[EvaluatedPart],
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    stage_matrix: Option<Affine2D>,
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
    let (stage_local, eff_bounds_size) = if let Some(sm) = stage_matrix {
        let sx = sm.a;
        let sy = sm.d;
        let local = Affine2D::translation(bounds_origin[0] * sx, bounds_origin[1] * sy)
            .mul(sm)
            .mul(flip_affine)
            .mul(shift_bounds);
        (local, [bounds_size[0] * sx.abs(), bounds_size[1] * sy.abs()])
    } else {
        (flip_affine.mul(shift_bounds), bounds_size)
    };
    let scale_affine = Affine2D::scale(params.scale[0], params.scale[1]);

    let rot_affine = if params.angle_deg.abs() > 1e-5 {
        let cx = eff_bounds_size[0] * params.scale[0] * 0.5;
        let cy = eff_bounds_size[1] * params.scale[1] * 0.5;
        Affine2D::translation(cx, cy)
            .mul(Affine2D::rotation(params.angle_deg.to_radians()))
            .mul(Affine2D::translation(-cx, -cy))
    } else {
        Affine2D::IDENTITY
    };

    let world_base = Affine2D::translation(untrimmed_x, untrimmed_y)
        .mul(rot_affine)
        .mul(scale_affine)
        .mul(stage_local);

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

        let sx_sq = m.a * m.a + m.b * m.b;
        let sy_sq = m.c * m.c + m.d * m.d;
        if sx_sq <= 1e-10 || sy_sq <= 1e-10 {
            continue;
        }

        let mut flags = SPRITE_FLAG_AFFINE;
        if part.rotated {
            flags |= SPRITE_FLAG_ROTATED_90_CCW;
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

        // Exact 2x2 affine quad (`pos` = m(0,0), `size` = m(w,0) - m(0,0), `origin` = m(0,h) - m(0,0)).
        let mut inst = SpriteInstance::new([m.tx, m.ty], [m.a * upright_w, m.b * upright_w])
            .with_origin([m.c * upright_h, m.d * upright_h])
            .with_uv_rect(uv_bounds)
            .with_color(tint)
            .with_flags(flags);
        if let Some([h, s, b, c]) = params.adjust_color {
            inst = inst.with_adjust_color(h, s, b, c);
        }

        batcher.push(tex, params.blend, filter, inst);
    }
}

pub(crate) fn upload_png_path(
    gpu: &mut GpuContext,
    label: Option<&str>,
    path: &Path,
) -> Option<(TextureHandle, [f32; 2])> {
    let tex = gpu.from_png_path_cached(label, path).ok()?;
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
    settings: Option<&TextureAtlasData>,
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
        apply_stage_matrix: settings.is_some_and(|s| s.apply_stage_matrix),
    })
}

fn compute_animate_bounds(atlas: &AnimateAtlas) -> ([f32; 2], [f32; 2]) {
    atlas.full_timeline_bounds()
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
        LoadedAtlasSource::Animate {
            atlas,
            bounds_origin: atlas_bounds_origin,
            bounds_size: atlas_bounds_size,
            apply_stage_matrix,
            ..
        } => {
            let prefix = anim.prefix.as_deref()?;
            let mode = if anim.anim_type.eq_ignore_ascii_case("symbol") {
                AnimateAnimMode::Symbol
            } else {
                AnimateAnimMode::FrameLabel
            };
            let (total_frames, (bounds_origin, bounds_size)) = match mode {
                AnimateAnimMode::FrameLabel => (
                    atlas.label_span(prefix).map_or(0, |s| s.duration),
                    (*atlas_bounds_origin, *atlas_bounds_size),
                ),
                AnimateAnimMode::Symbol => {
                    let total = atlas.symbol_total_frames(prefix).unwrap_or(0);
                    let sym_b = atlas
                        .symbol_bounds(prefix)
                        .unwrap_or((*atlas_bounds_origin, *atlas_bounds_size));
                    (total, sym_b)
                }
            };
            let stage_matrix = if *apply_stage_matrix {
                let c = atlas.stage_matrix().to_cols_array();
                Some(Affine2D {
                    a: c[0],
                    b: c[1],
                    c: c[2],
                    d: c[3],
                    tx: c[4],
                    ty: c[5],
                })
            } else {
                None
            };
            let indices: Vec<u32> = anim.frame_indices.iter().map(|&i| i as u32).collect();
            AnimFrames::Animate {
                atlas_idx,
                mode,
                target_name: prefix.to_owned(),
                indices,
                total_frames,
                bounds_origin,
                bounds_size,
                stage_matrix,
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

    pub(super) fn as_str(&self) -> &str {
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
const pub(super) fn dir_upper(dir: NoteDirection) -> &'static str {
    match dir {
        NoteDirection::Left => "LEFT",
        NoteDirection::Down => "DOWN",
        NoteDirection::Up => "UP",
        NoteDirection::Right => "RIGHT",
    }
}

/// Runtime V-Slice character controller (`BaseCharacter.hx` + Sparrow/Packer/Animate/Multi).
