//! Hardware-accelerated 2D renderer, atlas parsers, sustain mesh batching, and
//! zero-allocation bitmap/TTF text layout for Friday Night Funkin' V-Slice.

pub mod atlas;
pub mod batch;
pub mod gpu;
pub mod mesh_batch;
pub mod shaders;
pub mod text;

pub use atlas::{
    AnimateAtlas, AnimateSprite, AtlasError, AtlasFrame, EvaluatedPart, LabelSpan, PackerAtlas,
    PrefixFrames, SparrowAtlas, SpritemapInfo,
};
pub use batch::{
    compute_sprite_corner_uv, BlendMode, FilterMode, SpriteBatchQueue, SpriteBatcher,
    SpriteDrawGroup, SpriteInstance, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y,
    SPRITE_FLAG_PIXEL_SNAP, SPRITE_FLAG_ROTATED_90_CW,
};
pub use gpu::{
    compute_letterbox_viewport, orthographic_projection, GpuContext, GpuError, TextureHandle,
    ViewportRect,
};
pub use mesh_batch::{
    MeshBatchQueue, MeshBatcher, MeshDrawGroup, MeshVertex, SustainTrailParams,
    SUSTAIN_QUAD_INDICES,
};
pub use text::{
    format_hud_score, BitmapFont, FontAtlasImage, FontCase, FontError, Glyph, GlyphQuad,
    StackTextBuffer, TextAlign, TextStyle,
};
