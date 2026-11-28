//! Adobe Animate Texture Atlas parser (`Animation.json` + `spritemap*.json`)
//! and zero-allocation hierarchical frame evaluator.

use glam::{Affine2, Vec2};
use rustc_hash::FxHashMap;
use serde::Deserialize;

use super::AtlasError;

const MAX_SYMBOL_DEPTH: usize = 32;
const DEFAULT_FRAME_RATE: f32 = 24.0;

/// Frame span for a named label in the root Adobe Animate timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelSpan {
    /// First timeline frame index of this label (`I`).
    pub start: u32,
    /// Duration of this label in frames (`DU`).
    pub duration: u32,
}

/// Metadata for a single `spritemap*.json` texture sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpritemapInfo {
    /// Texture image filename from `meta.image`.
    pub image: String,
    /// Texture width in pixels from `meta.size.w`.
    pub width: u32,
    /// Texture height in pixels from `meta.size.h`.
    pub height: u32,
}

/// A sprite entry inside one of the `spritemap*.json` sheets.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimateSprite {
    /// Sprite identifier (`"name"` in `spritemap*.json`, referenced by `ASI.N`).
    pub name: String,
    /// Index of the owning spritemap in [`AnimateAtlas::spritemaps`].
    pub spritemap_index: usize,
    /// Left pixel coordinate in the spritemap texture.
    pub x: u32,
    /// Top pixel coordinate in the spritemap texture.
    pub y: u32,
    /// Packed sprite width in pixels.
    pub width: u32,
    /// Packed sprite height in pixels.
    pub height: u32,
    /// Whether the sprite is rotated 90 degrees in the spritemap texture.
    pub rotated: bool,
    /// Precomputed normalized `[u, v, u_width, v_height]`.
    pub uv_rect: [f32; 4],
}

/// A flattened, draw-ready sprite instance produced by [`AnimateAtlas::evaluate_frame`].
///
/// This type is `Copy` and contains no heap-owned fields so evaluation writes directly into a
/// caller-owned reusable `Vec<EvaluatedPart>` without allocating.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvaluatedPart {
    /// Index into [`AnimateAtlas::sprites`].
    pub sprite_index: usize,
    /// Index into [`AnimateAtlas::spritemaps`].
    pub spritemap_index: usize,
    /// Accumulated 2D affine transform from symbol root to sprite local coordinates.
    pub transform: Affine2,
    /// Normalized `[u, v, u_width, v_height]` into the owning spritemap texture.
    pub uv_rect: [f32; 4],
    /// Packed sprite dimensions `[width, height]` in pixels.
    pub size: [f32; 2],
    /// Whether the sprite is rotated in the spritemap texture.
    pub rotated: bool,
    /// Accumulated RGBA color multiplier `[r, g, b, a]`.
    pub color: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopMode {
    Loop,
    PlayOnce,
    SingleFrame,
}

#[derive(Debug, Clone)]
enum CompiledElement {
    Symbol {
        symbol_index: Option<usize>,
        first_frame: i32,
        loop_mode: LoopMode,
        transform: Affine2,
        color: [f32; 4],
    },
    Sprite {
        sprite_index: Option<usize>,
        transform: Affine2,
        color: [f32; 4],
    },
}

#[derive(Debug, Clone)]
struct CompiledKeyframe {
    index: u32,
    duration: u32,
    elements: Vec<CompiledElement>,
}

#[derive(Debug, Clone)]
struct CompiledLayer {
    is_mask: bool,
    frames: Vec<CompiledKeyframe>,
}

#[derive(Debug, Clone)]
struct CompiledTimeline {
    /// Layers stored in raw Animate top-to-bottom order; drawn in reverse (bottom-to-top).
    layers: Vec<CompiledLayer>,
    total_frames: u32,
}

/// Parsed and pre-compiled Adobe Animate Texture Atlas (`Animation.json` + `spritemap*.json`).
#[derive(Debug, Clone)]
pub struct AnimateAtlas {
    frame_rate: f32,
    stage_matrix: Affine2,
    main_symbol_name: Option<String>,
    spritemaps: Vec<SpritemapInfo>,
    sprites: Vec<AnimateSprite>,
    sprite_index: FxHashMap<String, usize>,
    main_timeline: CompiledTimeline,
    symbols: Vec<CompiledTimeline>,
    symbol_index: FxHashMap<String, usize>,
    labels: FxHashMap<String, LabelSpan>,
}

impl AnimateAtlas {
    /// Parses `Animation.json` and one or more `spritemap*.json` strings, pre-resolving all
    /// symbol names, sprite names, and frame labels at load time.
    pub fn from_json_strs(
        animation_json: &str,
        spritemaps_json: &[&str],
    ) -> Result<Self, AtlasError> {
        let animation_json = animation_json.trim_start_matches('\u{feff}');
        let raw_anim = parse_raw_animation_doc(animation_json)?;

        let mut spritemaps = Vec::with_capacity(spritemaps_json.len());
        let mut sprites = Vec::new();
        let mut sprite_index = FxHashMap::default();

        for (sm_idx, sm_json) in spritemaps_json.iter().enumerate() {
            let sm_json = sm_json.trim_start_matches('\u{feff}');
            let raw_sm: RawSpritemapDoc = serde_json::from_str(sm_json)?;
            let tex_w = raw_sm.meta.size.w;
            let tex_h = raw_sm.meta.size.h;
            spritemaps.push(SpritemapInfo {
                image: raw_sm.meta.image,
                width: tex_w,
                height: tex_h,
            });

            for entry in raw_sm.atlas.sprites {
                let s = entry.sprite;
                let uv_rect = if tex_w > 0 && tex_h > 0 {
                    [
                        s.x as f32 / tex_w as f32,
                        s.y as f32 / tex_h as f32,
                        s.w as f32 / tex_w as f32,
                        s.h as f32 / tex_h as f32,
                    ]
                } else {
                    [0.0; 4]
                };
                let idx = sprites.len();
                sprite_index.entry(s.name.clone()).or_insert(idx);
                sprites.push(AnimateSprite {
                    name: s.name,
                    spritemap_index: sm_idx,
                    x: s.x,
                    y: s.y,
                    width: s.w,
                    height: s.h,
                    rotated: s.rotated,
                    uv_rect,
                });
            }
        }

        let raw_symbols = raw_anim.sd.map(|sd| sd.symbols).unwrap_or_default();
        let mut symbol_index =
            FxHashMap::with_capacity_and_hasher(raw_symbols.len(), Default::default());
        for (idx, sym) in raw_symbols.iter().enumerate() {
            symbol_index.entry(sym.name.clone()).or_insert(idx);
        }

        let mut symbols = Vec::with_capacity(raw_symbols.len());
        for sym in &raw_symbols {
            symbols.push(compile_timeline(
                &sym.timeline,
                &symbol_index,
                &sprite_index,
                None,
            ));
        }

        let mut labels = FxHashMap::default();
        let main_timeline = compile_timeline(
            &raw_anim.an.timeline,
            &symbol_index,
            &sprite_index,
            Some(&mut labels),
        );

        let frame_rate = raw_anim
            .md
            .and_then(|md| md.frt)
            .filter(|&fps| fps > 0.0)
            .unwrap_or(DEFAULT_FRAME_RATE);
        let stage_matrix = raw_anim
            .an
            .stage_instance
            .and_then(|sti| sti.si)
            .map(|si| extract_affine(si.mx, si.m3d, si.pos))
            .unwrap_or(Affine2::IDENTITY);

        let main_symbol_name = raw_anim.an.symbol_name;
        Ok(Self {
            frame_rate,
            stage_matrix,
            main_symbol_name,
            spritemaps,
            sprites,
            sprite_index,
            main_timeline,
            symbols,
            symbol_index,
            labels,
        })
    }

    /// Stage placement transform parsed from `AN.STI.SI` (identity if absent).
    #[inline]
    #[must_use]
    pub fn stage_matrix(&self) -> Affine2 {
        self.stage_matrix
    }

    /// Timeline frame rate from `MD.FRT` (defaults to `24.0`).
    #[inline]
    #[must_use]
    pub fn frame_rate(&self) -> f32 {
        self.frame_rate
    }

    /// Total number of frames in the root animation timeline.
    #[inline]
    #[must_use]
    pub fn total_frames(&self) -> u32 {
        self.main_timeline.total_frames
    }

    /// Loaded spritemap texture metadata in order of `spritemaps_json`.
    #[inline]
    #[must_use]
    pub fn spritemaps(&self) -> &[SpritemapInfo] {
        &self.spritemaps
    }

    /// All parsed sprite entries across all spritemaps.
    #[inline]
    #[must_use]
    pub fn sprites(&self) -> &[AnimateSprite] {
        &self.sprites
    }

    /// Looks up a sprite entry by name in `O(1)` time without allocating.
    #[inline]
    #[must_use]
    pub fn sprite_by_name(&self, name: &str) -> Option<&AnimateSprite> {
        self.sprite_index.get(name).map(|&idx| &self.sprites[idx])
    }

    /// Looks up the timeline span for a frame label in `O(1)` time without allocating.
    #[inline]
    #[must_use]
    pub fn label_span(&self, label: &str) -> Option<LabelSpan> {
        self.labels.get(label).copied()
    }

    fn timeline_for_symbol(&self, symbol_name: &str) -> Option<&CompiledTimeline> {
        if let Some(&sym_idx) = self.symbol_index.get(symbol_name) {
            return Some(&self.symbols[sym_idx]);
        }
        if self.main_symbol_name.as_deref() == Some(symbol_name) {
            return Some(&self.main_timeline);
        }
        None
    }

    /// Returns the total frame count of a named symbol in `SD.S` (or `AN.SN` main timeline).
    #[inline]
    #[must_use]
    pub fn symbol_total_frames(&self, symbol_name: &str) -> Option<u32> {
        self.timeline_for_symbol(symbol_name)
            .map(|t| t.total_frames)
    }

    /// Computes union `[origin, size]` bounding box over all frames of the main timeline (`AN.TL`).
    #[must_use]
    pub fn full_timeline_bounds(&self) -> ([f32; 2], [f32; 2]) {
        self.compute_timeline_bounds(&self.main_timeline)
            .unwrap_or(([0.0, 0.0], [1.0, 1.0]))
    }

    /// Computes union `[origin, size]` bounding box over all frames of a symbol inside `SD.S` (or `AN.SN`).
    #[must_use]
    pub fn symbol_bounds(&self, symbol_name: &str) -> Option<([f32; 2], [f32; 2])> {
        let timeline = self.timeline_for_symbol(symbol_name)?;
        self.compute_timeline_bounds(timeline)
    }

    fn compute_timeline_bounds(&self, timeline: &CompiledTimeline) -> Option<([f32; 2], [f32; 2])> {
        let total = timeline.total_frames.max(1);
        let mut parts = Vec::with_capacity(64);
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for frame_idx in 0..total {
            parts.clear();
            self.eval_timeline(timeline, frame_idx, EvalCtx::ROOT, &mut parts);
            for part in &parts {
                let (w, h) = if part.rotated {
                    (part.size[1], part.size[0])
                } else {
                    (part.size[0], part.size[1])
                };
                let m = part.transform;
                for &(lx, ly) in &[(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
                    let pt = m.transform_point2(Vec2::new(lx, ly));
                    min_x = min_x.min(pt.x);
                    min_y = min_y.min(pt.y);
                    max_x = max_x.max(pt.x);
                    max_y = max_y.max(pt.y);
                }
            }
        }

        if !min_x.is_finite() || !min_y.is_finite() {
            return None;
        }
        Some((
            [min_x, min_y],
            [(max_x - min_x).max(1.0), (max_y - min_y).max(1.0)],
        ))
    }

    /// Evaluates the root timeline at `frame_idx`, clearing `out_parts` first and appending
    /// flattened drawable parts in back-to-front draw order with zero heap allocations.
    pub fn evaluate_frame(&self, frame_idx: u32, out_parts: &mut Vec<EvaluatedPart>) {
        out_parts.clear();
        self.eval_timeline(&self.main_timeline, frame_idx, EvalCtx::ROOT, out_parts);
    }

    /// Evaluates a labeled frame span (e.g. `"Idle"`) at `local_frame` (clamped within the
    /// label's duration), clearing `out_parts` first with zero heap allocations.
    ///
    /// Returns `false` (leaving `out_parts` cleared) if `label` is unknown.
    pub fn evaluate_label(
        &self,
        label: &str,
        local_frame: u32,
        out_parts: &mut Vec<EvaluatedPart>,
    ) -> bool {
        out_parts.clear();
        let Some(&span) = self.labels.get(label) else {
            return false;
        };
        let offset = if span.duration > 0 {
            local_frame.min(span.duration - 1)
        } else {
            0
        };
        self.eval_timeline(
            &self.main_timeline,
            span.start + offset,
            EvalCtx::ROOT,
            out_parts,
        );
        true
    }

    /// Evaluates a named symbol from `SD.S` (or `AN.SN` main timeline) directly at `local_frame`,
    /// clearing `out_parts` first with zero heap allocations. Returns `false` if `symbol_name` is unknown.
    pub fn evaluate_symbol(
        &self,
        symbol_name: &str,
        local_frame: u32,
        out_parts: &mut Vec<EvaluatedPart>,
    ) -> bool {
        out_parts.clear();
        let Some(timeline) = self.timeline_for_symbol(symbol_name) else {
            return false;
        };
        let clamped = local_frame.min(timeline.total_frames.saturating_sub(1));
        self.eval_timeline(timeline, clamped, EvalCtx::ROOT, out_parts);
        true
    }

    fn eval_timeline(
        &self,
        timeline: &CompiledTimeline,
        frame_idx: u32,
        ctx: EvalCtx,
        out_parts: &mut Vec<EvaluatedPart>,
    ) {
        if ctx.depth >= MAX_SYMBOL_DEPTH {
            return;
        }

        // Layers in Animate JSON are ordered top-to-bottom, so iterate in reverse for painter's
        // draw order (bottom layer drawn first). Skip mask/clipper/guide layers (`LT: "Clp"`).
        for layer in timeline.layers.iter().rev() {
            if layer.is_mask {
                continue;
            }
            let Some(kf) = find_keyframe(&layer.frames, frame_idx) else {
                continue;
            };
            let elapsed = frame_idx - kf.index;

            for elem in &kf.elements {
                match elem {
                    CompiledElement::Sprite {
                        sprite_index: Some(s_idx),
                        transform,
                        color,
                    } => {
                        let sprite = &self.sprites[*s_idx];
                        out_parts.push(EvaluatedPart {
                            sprite_index: *s_idx,
                            spritemap_index: sprite.spritemap_index,
                            transform: ctx.transform * *transform,
                            uv_rect: sprite.uv_rect,
                            size: [sprite.width as f32, sprite.height as f32],
                            rotated: sprite.rotated,
                            color: mul_color(ctx.color, *color),
                        });
                    }
                    CompiledElement::Sprite {
                        sprite_index: None, ..
                    } => {}
                    CompiledElement::Symbol {
                        symbol_index: Some(sym_idx),
                        first_frame,
                        loop_mode,
                        transform,
                        color,
                    } => {
                        let child_tl = &self.symbols[*sym_idx];
                        let child_frame = resolve_child_frame(
                            *first_frame,
                            elapsed,
                            *loop_mode,
                            child_tl.total_frames,
                        );
                        let child_ctx = EvalCtx {
                            transform: ctx.transform * *transform,
                            color: mul_color(ctx.color, *color),
                            depth: ctx.depth + 1,
                        };
                        self.eval_timeline(child_tl, child_frame, child_ctx, out_parts);
                    }
                    CompiledElement::Symbol {
                        symbol_index: None, ..
                    } => {}
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct EvalCtx {
    transform: Affine2,
    color: [f32; 4],
    depth: usize,
}

impl EvalCtx {
    const ROOT: Self = Self {
        transform: Affine2::IDENTITY,
        color: [1.0; 4],
        depth: 0,
    };
}

#[inline]
fn find_keyframe(frames: &[CompiledKeyframe], frame_idx: u32) -> Option<&CompiledKeyframe> {
    let pos = frames.partition_point(|kf| kf.index <= frame_idx);
    if pos == 0 {
        return None;
    }
    let kf = &frames[pos - 1];
    if frame_idx < kf.index.saturating_add(kf.duration) {
        Some(kf)
    } else {
        None
    }
}

#[inline]
fn resolve_child_frame(first_frame: i32, elapsed: u32, mode: LoopMode, total_frames: u32) -> u32 {
    let total = total_frames.max(1);
    match mode {
        LoopMode::SingleFrame => first_frame.clamp(0, (total - 1) as i32) as u32,
        LoopMode::PlayOnce => {
            let raw = first_frame.saturating_add(elapsed as i32);
            raw.clamp(0, (total - 1) as i32) as u32
        }
        LoopMode::Loop => {
            let raw = first_frame.saturating_add(elapsed as i32);
            raw.rem_euclid(total as i32) as u32
        }
    }
}

#[inline]
fn mul_color(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
}

fn compile_timeline(
    raw: &RawTimeline,
    symbol_index: &FxHashMap<String, usize>,
    sprite_index: &FxHashMap<String, usize>,
    mut labels_out: Option<&mut FxHashMap<String, LabelSpan>>,
) -> CompiledTimeline {
    let mut layers = Vec::with_capacity(raw.layers.len());
    let mut max_end = 0u32;

    for raw_layer in &raw.layers {
        let mut frames = Vec::with_capacity(raw_layer.frames.len());
        for raw_kf in &raw_layer.frames {
            if let Some(ref mut labels) = labels_out {
                if let Some(name) = raw_kf.name.as_deref().filter(|s| !s.is_empty()) {
                    let span = LabelSpan {
                        start: raw_kf.index,
                        duration: raw_kf.duration,
                    };
                    labels.entry(name.to_owned()).or_insert(span);
                    // Some V-Slice Animate files have trailing whitespace in labels (e.g. "up ", "down ").
                    let trimmed = name.trim();
                    if trimmed != name && !trimmed.is_empty() {
                        labels.entry(trimmed.to_owned()).or_insert(span);
                    }
                }
            }

            let end = raw_kf.index.saturating_add(raw_kf.duration);
            if end > max_end {
                max_end = end;
            }

            let mut elements = Vec::with_capacity(raw_kf.elements.len());
            for raw_elem in &raw_kf.elements {
                match raw_elem {
                    RawElement::Symbol { si } => {
                        let loop_mode = match si.lp.as_deref() {
                            Some("PO") => LoopMode::PlayOnce,
                            Some("SF") => LoopMode::SingleFrame,
                            _ => LoopMode::Loop,
                        };
                        elements.push(CompiledElement::Symbol {
                            symbol_index: symbol_index.get(&si.sn).copied(),
                            first_frame: si.ff,
                            loop_mode,
                            transform: extract_affine(si.mx, si.m3d, si.pos),
                            color: extract_color(si.color.as_ref()),
                        });
                    }
                    RawElement::Sprite { asi } => {
                        elements.push(CompiledElement::Sprite {
                            sprite_index: sprite_index.get(&asi.name).copied(),
                            transform: extract_affine(asi.mx, asi.m3d, asi.pos),
                            color: extract_color(asi.color.as_ref()),
                        });
                    }
                }
            }

            frames.push(CompiledKeyframe {
                index: raw_kf.index,
                duration: raw_kf.duration,
                elements,
            });
        }

        frames.sort_by_key(|kf| kf.index);
        let is_mask = matches!(
            raw_layer.layer_type.as_deref(),
            Some("Clp" | "Clipper" | "Gd" | "Guide")
        );
        layers.push(CompiledLayer { is_mask, frames });
    }

    CompiledTimeline {
        layers,
        total_frames: max_end.max(1),
    }
}

fn extract_affine(
    mx: Option<[f32; 6]>,
    m3d: Option<[f32; 16]>,
    pos: Option<RawPoint>,
) -> Affine2 {
    if let Some(m) = mx {
        return Affine2::from_cols_array(&m);
    }
    if let Some(m) = m3d {
        return Affine2::from_cols_array(&[m[0], m[1], m[4], m[5], m[12], m[13]]);
    }
    if let Some(p) = pos {
        return Affine2::from_translation(Vec2::new(p.x, p.y));
    }
    Affine2::IDENTITY
}

fn extract_color(color: Option<&RawColorEffect>) -> [f32; 4] {
    let Some(c) = color else {
        return [1.0; 4];
    };
    match c.mode.as_deref() {
        Some("CA") => [1.0, 1.0, 1.0, c.am.unwrap_or(1.0)],
        Some("AD") => [
            c.rm.unwrap_or(1.0),
            c.gm.unwrap_or(1.0),
            c.bm.unwrap_or(1.0),
            c.am.unwrap_or(1.0),
        ],
        Some("T") => {
            if let Some(hex) = c.tc.as_deref().and_then(parse_hex_rgb) {
                let m = c.tm.unwrap_or(1.0).clamp(0.0, 1.0);
                [
                    1.0 + (hex[0] - 1.0) * m,
                    1.0 + (hex[1] - 1.0) * m,
                    1.0 + (hex[2] - 1.0) * m,
                    c.am.unwrap_or(1.0),
                ]
            } else {
                [1.0, 1.0, 1.0, c.am.unwrap_or(1.0)]
            }
        }
        _ => [
            c.rm.unwrap_or(1.0),
            c.gm.unwrap_or(1.0),
            c.bm.unwrap_or(1.0),
            c.am.unwrap_or(1.0),
        ],
    }
}

fn parse_hex_rgb(s: &str) -> Option<[f32; 3]> {
    let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if hex.len() != 6 {
        return None;
    }
    let val = u32::from_str_radix(hex, 16).ok()?;
    let r = ((val >> 16) & 0xFF) as f32 / 255.0;
    let g = ((val >> 8) & 0xFF) as f32 / 255.0;
    let b = (val & 0xFF) as f32 / 255.0;
    Some([r, g, b])
}

fn parse_raw_animation_doc(json_str: &str) -> Result<RawAnimationDoc, AtlasError> {
    let val: serde_json::Value = serde_json::from_str(json_str)?;
    if val.get("AN").is_some() {
        return Ok(serde_json::from_value(val)?);
    }
    let legacy: LegacyAnimationDoc = serde_json::from_value(val)?;
    Ok(legacy.into_raw())
}

#[derive(Deserialize)]
struct LegacyAnimationDoc {
    #[serde(rename = "ANIMATION")]
    animation: LegacyAnimationNode,
    #[serde(rename = "SYMBOL_DICTIONARY", default)]
    symbol_dictionary: Option<LegacySymbolDict>,
    #[serde(rename = "metadata", default)]
    metadata: Option<LegacyMetadata>,
}

#[derive(Deserialize)]
struct LegacyMetadata {
    #[serde(default)]
    framerate: Option<f32>,
}

#[derive(Deserialize)]
struct LegacyAnimationNode {
    #[serde(rename = "TIMELINE")]
    timeline: LegacyTimeline,
    #[serde(rename = "StageInstance", default)]
    stage_instance: Option<LegacyStageInstance>,
}

#[derive(Deserialize)]
struct LegacyStageInstance {
    #[serde(rename = "SYMBOL_Instance", default)]
    si: Option<LegacySymbolInstance>,
}

#[derive(Deserialize)]
struct LegacySymbolDict {
    #[serde(rename = "Symbols", default)]
    symbols: Vec<LegacySymbolDef>,
}

#[derive(Deserialize)]
struct LegacySymbolDef {
    #[serde(rename = "SYMBOL_name")]
    name: String,
    #[serde(rename = "TIMELINE")]
    timeline: LegacyTimeline,
}

#[derive(Deserialize)]
struct LegacyTimeline {
    #[serde(rename = "LAYERS", default)]
    layers: Vec<LegacyLayer>,
}

#[derive(Deserialize)]
struct LegacyLayer {
    #[serde(rename = "Layer_type", default)]
    layer_type: Option<String>,
    #[serde(rename = "Frames", default)]
    frames: Vec<LegacyKeyframe>,
}

#[derive(Deserialize)]
struct LegacyKeyframe {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    index: u32,
    #[serde(default = "default_duration")]
    duration: u32,
    #[serde(default)]
    elements: Vec<LegacyElement>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LegacyElement {
    Symbol {
        #[serde(rename = "SYMBOL_Instance")]
        si: LegacySymbolInstance,
    },
    Sprite {
        #[serde(rename = "ATLAS_SPRITE_instance")]
        asi: LegacyAtlasSpriteInstance,
    },
}

#[derive(Deserialize)]
struct LegacySymbolInstance {
    #[serde(rename = "SYMBOL_name")]
    sn: String,
    #[serde(rename = "firstFrame", default)]
    ff: i32,
    #[serde(rename = "loop", default)]
    lp: Option<String>,
    #[serde(rename = "Matrix3D", default)]
    m3d: Option<LegacyMatrix3D>,
    #[serde(rename = "color", default)]
    color: Option<LegacyColorEffect>,
}

#[derive(Deserialize)]
struct LegacyAtlasSpriteInstance {
    name: String,
    #[serde(rename = "Matrix3D", default)]
    m3d: Option<LegacyMatrix3D>,
    #[serde(rename = "Position", default)]
    pos: Option<RawPoint>,
    #[serde(rename = "color", default)]
    color: Option<LegacyColorEffect>,
}

#[derive(Clone, Copy, Deserialize)]
struct LegacyMatrix3D {
    #[serde(default = "one_f32")]
    m00: f32,
    #[serde(default)]
    m01: f32,
    #[serde(default)]
    m10: f32,
    #[serde(default = "one_f32")]
    m11: f32,
    #[serde(default)]
    m30: f32,
    #[serde(default)]
    m31: f32,
}

fn one_f32() -> f32 {
    1.0
}

impl LegacyMatrix3D {
    fn into_mx(self) -> [f32; 6] {
        [self.m00, self.m01, self.m10, self.m11, self.m30, self.m31]
    }
}

#[derive(Deserialize)]
struct LegacyColorEffect {
    #[serde(default)]
    mode: Option<String>,
    #[serde(rename = "alphaMultiplier", default)]
    am: Option<f32>,
    #[serde(rename = "redMultiplier", default)]
    rm: Option<f32>,
    #[serde(rename = "greenMultiplier", default)]
    gm: Option<f32>,
    #[serde(rename = "blueMultiplier", default)]
    bm: Option<f32>,
    #[serde(rename = "tintColor", default)]
    tc: Option<String>,
    #[serde(rename = "tintMultiplier", default)]
    tm: Option<f32>,
}

impl LegacyColorEffect {
    fn into_raw(self) -> RawColorEffect {
        RawColorEffect {
            mode: self.mode,
            am: self.am,
            rm: self.rm,
            gm: self.gm,
            bm: self.bm,
            tc: self.tc,
            tm: self.tm,
        }
    }
}

impl LegacyTimeline {
    fn into_raw(self) -> RawTimeline {
        RawTimeline {
            layers: self
                .layers
                .into_iter()
                .map(|l| RawLayer {
                    layer_type: l.layer_type,
                    frames: l
                        .frames
                        .into_iter()
                        .map(|f| RawKeyframe {
                            name: f.name,
                            index: f.index,
                            duration: f.duration,
                            elements: f
                                .elements
                                .into_iter()
                                .map(|el| match el {
                                    LegacyElement::Symbol { si } => {
                                        let lp = si.lp.as_deref().map(|s| {
                                            if s.eq_ignore_ascii_case("playonce") {
                                                "PO".to_owned()
                                            } else if s.eq_ignore_ascii_case("singleframe") {
                                                "SF".to_owned()
                                            } else {
                                                "LP".to_owned()
                                            }
                                        });
                                        RawElement::Symbol {
                                            si: RawSymbolInstance {
                                                sn: si.sn,
                                                ff: si.ff,
                                                lp,
                                                mx: si.m3d.map(LegacyMatrix3D::into_mx),
                                                m3d: None,
                                                pos: None,
                                                color: si.color.map(LegacyColorEffect::into_raw),
                                            },
                                        }
                                    }
                                    LegacyElement::Sprite { asi } => RawElement::Sprite {
                                        asi: RawAtlasSpriteInstance {
                                            name: asi.name,
                                            mx: asi.m3d.map(LegacyMatrix3D::into_mx),
                                            m3d: None,
                                            pos: asi.pos,
                                            color: asi.color.map(LegacyColorEffect::into_raw),
                                        },
                                    },
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl LegacyAnimationDoc {
    fn into_raw(self) -> RawAnimationDoc {
        let stage_instance = self.animation.stage_instance.and_then(|sti| {
            sti.si.map(|si| RawStageInstance {
                si: Some(RawSymbolInstance {
                    sn: si.sn,
                    ff: si.ff,
                    lp: si.lp,
                    mx: None,
                    m3d: si.m3d.map(LegacyMatrix3D::into_mx).map(|m| {
                        [
                            m[0], m[1], 0.0, 0.0, m[2], m[3], 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                            m[4], m[5], 0.0, 1.0,
                        ]
                    }),
                    pos: None,
                    color: si.color.map(LegacyColorEffect::into_raw),
                }),
            })
        });
        RawAnimationDoc {
            an: RawAnimationNode {
                symbol_name: None,
                timeline: self.animation.timeline.into_raw(),
                stage_instance,
            },
            sd: self.symbol_dictionary.map(|sd| RawSymbolDict {
                symbols: sd
                    .symbols
                    .into_iter()
                    .map(|s| RawSymbolDef {
                        name: s.name,
                        timeline: s.timeline.into_raw(),
                    })
                    .collect(),
            }),
            md: self
                .metadata
                .map(|md| RawMetadata { frt: md.framerate }),
        }
    }
}

#[derive(Deserialize)]
struct RawAnimationDoc {
    #[serde(rename = "AN")]
    an: RawAnimationNode,
    #[serde(rename = "SD", default)]
    sd: Option<RawSymbolDict>,
    #[serde(rename = "MD", default)]
    md: Option<RawMetadata>,
}

#[derive(Deserialize)]
struct RawMetadata {
    #[serde(rename = "FRT", default)]
    frt: Option<f32>,
}

#[derive(Deserialize)]
struct RawAnimationNode {
    #[serde(rename = "SN", default)]
    symbol_name: Option<String>,
    #[serde(rename = "TL")]
    timeline: RawTimeline,
    #[serde(rename = "STI", default)]
    stage_instance: Option<RawStageInstance>,
}

#[derive(Deserialize)]
struct RawStageInstance {
    #[serde(rename = "SI", default)]
    si: Option<RawSymbolInstance>,
}

#[derive(Deserialize)]
struct RawSymbolDict {
    #[serde(rename = "S", default)]
    symbols: Vec<RawSymbolDef>,
}

#[derive(Deserialize)]
struct RawSymbolDef {
    #[serde(rename = "SN")]
    name: String,
    #[serde(rename = "TL")]
    timeline: RawTimeline,
}

#[derive(Deserialize)]
struct RawTimeline {
    #[serde(rename = "L", default)]
    layers: Vec<RawLayer>,
}

#[derive(Deserialize)]
struct RawLayer {
    #[serde(rename = "LT", default)]
    layer_type: Option<String>,
    #[serde(rename = "FR", default)]
    frames: Vec<RawKeyframe>,
}

fn default_duration() -> u32 {
    1
}

#[derive(Deserialize)]
struct RawKeyframe {
    #[serde(rename = "N", default)]
    name: Option<String>,
    #[serde(rename = "I", default)]
    index: u32,
    #[serde(rename = "DU", default = "default_duration")]
    duration: u32,
    #[serde(rename = "E", default)]
    elements: Vec<RawElement>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawElement {
    Symbol {
        #[serde(rename = "SI")]
        si: RawSymbolInstance,
    },
    Sprite {
        #[serde(rename = "ASI")]
        asi: RawAtlasSpriteInstance,
    },
}

#[derive(Deserialize)]
struct RawSymbolInstance {
    #[serde(rename = "SN")]
    sn: String,
    #[serde(rename = "FF", default)]
    ff: i32,
    #[serde(rename = "LP", default)]
    lp: Option<String>,
    #[serde(rename = "MX", default)]
    mx: Option<[f32; 6]>,
    #[serde(rename = "M3D", default)]
    m3d: Option<[f32; 16]>,
    #[serde(rename = "POS", default)]
    pos: Option<RawPoint>,
    #[serde(rename = "C", default)]
    color: Option<RawColorEffect>,
}

#[derive(Deserialize)]
struct RawAtlasSpriteInstance {
    #[serde(rename = "N")]
    name: String,
    #[serde(rename = "MX", default)]
    mx: Option<[f32; 6]>,
    #[serde(rename = "M3D", default)]
    m3d: Option<[f32; 16]>,
    #[serde(rename = "POS", default)]
    pos: Option<RawPoint>,
    #[serde(rename = "C", default)]
    color: Option<RawColorEffect>,
}

#[derive(Clone, Copy, Deserialize)]
struct RawPoint {
    x: f32,
    y: f32,
}

#[derive(Deserialize)]
struct RawColorEffect {
    #[serde(rename = "M", default)]
    mode: Option<String>,
    #[serde(rename = "AM", default)]
    am: Option<f32>,
    #[serde(rename = "RM", default)]
    rm: Option<f32>,
    #[serde(rename = "GM", default)]
    gm: Option<f32>,
    #[serde(rename = "BM", default)]
    bm: Option<f32>,
    #[serde(rename = "TC", default)]
    tc: Option<String>,
    #[serde(rename = "TM", default)]
    tm: Option<f32>,
}

#[derive(Deserialize)]
struct RawSpritemapDoc {
    #[serde(rename = "ATLAS")]
    atlas: RawSpritemapAtlas,
    #[serde(rename = "meta")]
    meta: RawSpritemapMeta,
}

#[derive(Deserialize)]
struct RawSpritemapAtlas {
    #[serde(rename = "SPRITES", default)]
    sprites: Vec<RawSpritemapEntry>,
}

#[derive(Deserialize)]
struct RawSpritemapEntry {
    #[serde(rename = "SPRITE")]
    sprite: RawSpritemapSprite,
}

#[derive(Deserialize)]
struct RawSpritemapSprite {
    name: String,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    #[serde(default)]
    rotated: bool,
}

#[derive(Deserialize)]
struct RawSpritemapMeta {
    image: String,
    size: RawSpritemapSize,
}

#[derive(Deserialize)]
struct RawSpritemapSize {
    w: u32,
    h: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ANIMATION_JSON: &str = r#"{
        "AN": {
            "N": "BoyFriend Assets_TA-Export",
            "SN": "BF ALL ANIMS",
            "TL": {
                "L": [
                    {
                        "LN": "Labels",
                        "FR": [
                            { "N": "Idle", "I": 0, "DU": 4, "E": [] },
                            { "N": "Left", "I": 4, "DU": 2, "E": [] }
                        ]
                    },
                    {
                        "LN": "Layer_1",
                        "FR": [
                            {
                                "I": 0,
                                "DU": 4,
                                "E": [
                                    {
                                        "SI": {
                                            "SN": "BF idle dance",
                                            "FF": 0,
                                            "ST": "G",
                                            "LP": "LP",
                                            "MX": [1, 0, 0, 1, -500, 0]
                                        }
                                    }
                                ]
                            },
                            {
                                "I": 4,
                                "DU": 2,
                                "E": [
                                    {
                                        "SI": {
                                            "SN": "BF left",
                                            "FF": 0,
                                            "ST": "G",
                                            "LP": "PO",
                                            "MX": [1, 0, 0, 1, -480, 10],
                                            "C": { "M": "CA", "AM": 0.5 }
                                        }
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }
        },
        "SD": {
            "S": [
                {
                    "SN": "BF idle dance",
                    "TL": {
                        "L": [
                            {
                                "LN": "Head",
                                "FR": [
                                    {
                                        "I": 0,
                                        "DU": 2,
                                        "E": [
                                            { "ASI": { "MX": [1, 0, 0, 1, 10, 20], "N": "0" } }
                                        ]
                                    },
                                    {
                                        "I": 2,
                                        "DU": 2,
                                        "E": [
                                            { "ASI": { "MX": [1, 0, 0, 1, 14, 24], "N": "1" } }
                                        ]
                                    }
                                ]
                            },
                            {
                                "LN": "Body",
                                "FR": [
                                    {
                                        "I": 0,
                                        "DU": 4,
                                        "E": [
                                            { "ASI": { "MX": [1, 0, 0, 1, 0, 100], "N": "2" } }
                                        ]
                                    }
                                ]
                            }
                        ]
                    }
                },
                {
                    "SN": "BF left",
                    "TL": {
                        "L": [
                            {
                                "LN": "Layer 1",
                                "FR": [
                                    {
                                        "I": 0,
                                        "DU": 2,
                                        "E": [
                                            { "ASI": { "MX": [2, 0, 0, 2, 5, 5], "N": "3" } }
                                        ]
                                    }
                                ]
                            }
                        ]
                    }
                }
            ]
        },
        "MD": { "FRT": 24 }
    }"#;

    const SAMPLE_SPRITEMAP_1: &str = r#"{
        "ATLAS": {
            "SPRITES": [
                { "SPRITE": { "name": "0", "x": 100, "y": 50, "w": 160, "h": 140, "rotated": false } },
                { "SPRITE": { "name": "1", "x": 300, "y": 50, "w": 160, "h": 140, "rotated": false } },
                { "SPRITE": { "name": "2", "x": 0, "y": 200, "w": 200, "h": 220, "rotated": true } }
            ]
        },
        "meta": {
            "image": "spritemap1.png",
            "size": { "w": 1000, "h": 500 }
        }
    }"#;

    const SAMPLE_SPRITEMAP_2: &str = r#"{
        "ATLAS": {
            "SPRITES": [
                { "SPRITE": { "name": "3", "x": 10, "y": 20, "w": 80, "h": 90, "rotated": false } }
            ]
        },
        "meta": {
            "image": "spritemap2.png",
            "size": { "w": 512, "h": 512 }
        }
    }"#;

    #[test]
    fn parses_multi_spritemap_and_evaluates_hierarchy() {
        let atlas = AnimateAtlas::from_json_strs(
            SAMPLE_ANIMATION_JSON,
            &[SAMPLE_SPRITEMAP_1, SAMPLE_SPRITEMAP_2],
        )
        .expect("parse animate atlas");

        assert!((atlas.frame_rate() - 24.0).abs() < 1e-6);
        assert_eq!(atlas.total_frames(), 6);
        assert_eq!(atlas.spritemaps().len(), 2);
        assert_eq!(atlas.sprites().len(), 4);

        let idle_span = atlas.label_span("Idle").expect("Idle label");
        assert_eq!(idle_span, LabelSpan { start: 0, duration: 4 });

        let mut parts = Vec::with_capacity(16);
        let initial_ptr = parts.as_ptr();
        let initial_cap = parts.capacity();

        // Evaluate Idle at local frame 2 -> Body ("2") then Head ("1").
        assert!(atlas.evaluate_label("Idle", 2, &mut parts));
        assert_eq!(parts.len(), 2);

        // Bottom layer ("Body") drawn before top layer ("Head").
        assert_eq!(atlas.sprites()[parts[0].sprite_index].name, "2");
        assert!(parts[0].rotated);
        assert_eq!(parts[0].spritemap_index, 0);
        assert!((parts[0].transform.translation.x - (-500.0)).abs() < 1e-5);
        assert!((parts[0].transform.translation.y - 100.0).abs() < 1e-5);

        assert_eq!(atlas.sprites()[parts[1].sprite_index].name, "1");
        assert!((parts[1].transform.translation.x - (-486.0)).abs() < 1e-5);
        assert!((parts[1].transform.translation.y - 24.0).abs() < 1e-5);

        // Evaluate Left (on spritemap2, with alpha 0.5 and 2x scale).
        assert!(atlas.evaluate_label("Left", 0, &mut parts));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].spritemap_index, 1);
        assert!((parts[0].color[3] - 0.5).abs() < 1e-5);
        assert!((parts[0].transform.translation.x - (-475.0)).abs() < 1e-5);
        assert!((parts[0].transform.translation.y - 15.0).abs() < 1e-5);

        // Verify zero heap reallocations occurred on the caller-owned buffer.
        assert_eq!(parts.as_ptr(), initial_ptr);
        assert_eq!(parts.capacity(), initial_cap);
    }

    #[test]
    fn unknown_label_returns_false_and_clears_buffer() {
        let atlas =
            AnimateAtlas::from_json_strs(SAMPLE_ANIMATION_JSON, &[SAMPLE_SPRITEMAP_1]).unwrap();
        let mut parts = Vec::with_capacity(8);
        atlas.evaluate_frame(0, &mut parts);
        assert!(!parts.is_empty());

        assert!(!atlas.evaluate_label("NonExistent", 0, &mut parts));
        assert!(parts.is_empty());
        assert!((atlas.stage_matrix().translation.x).abs() < 1e-6);
    }

    #[test]
    fn parses_stage_instance_matrix() {
        let doc = r#"{
            "AN": {
                "STI": {
                    "SI": {
                        "SN": "rando",
                        "MX": [1.0, 0.0, 0.0, 1.0, 1435.6, 560.4]
                    }
                },
                "TL": { "L": [] }
            },
            "MD": { "FRT": 24 }
        }"#;
        let atlas = AnimateAtlas::from_json_strs(doc, &[SAMPLE_SPRITEMAP_1]).expect("parse");
        let sm = atlas.stage_matrix();
        assert!((sm.translation.x - 1435.6).abs() < 1e-4);
        assert!((sm.translation.y - 560.4).abs() < 1e-4);
        assert!((sm.matrix2.x_axis.x - 1.0).abs() < 1e-6);
        assert!((sm.matrix2.y_axis.y - 1.0).abs() < 1e-6);
    }
}
