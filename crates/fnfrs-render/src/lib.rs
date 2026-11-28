//! Hardware-accelerated 2D renderer, atlas parsers, sustain mesh batching, and
//! zero-allocation bitmap/TTF text layout for Friday Night Funkin' V-Slice.

pub mod atlas;
pub mod atlas_text;
pub mod batch;
pub mod drop_shadow;
pub mod gpu;
pub mod mesh_batch;
pub mod rain;
pub mod shaders;
pub mod text;

pub use atlas::{
    AnimateAtlas, AnimateSprite, AtlasError, AtlasFrame, EvaluatedPart, LabelSpan, PackerAtlas,
    PrefixFrames, SparrowAtlas, SpritemapInfo,
};
pub use atlas_text::{AtlasFont, AtlasTextStyle};
pub use batch::{
    compute_sprite_corner_uv, BlendMode, FilterMode, SpriteBatchQueue, SpriteBatcher,
    SpriteDrawGroup, SpriteInstance, SPRITE_FLAG_ADJUST_COLOR, SPRITE_FLAG_AFFINE,
    SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y, SPRITE_FLAG_PIXEL_SNAP, SPRITE_FLAG_RAIN,
    SPRITE_FLAG_ROTATED_90_CCW, SPRITE_FLAG_ROTATED_90_CW, SPRITE_FLAG_WIGGLE,
};
pub use drop_shadow::DropShadowParams;
pub use gpu::{
    compute_letterbox_viewport, orthographic_projection, zoomed_orthographic_projection, GpuContext,
    GpuError, TextureHandle, ViewportRect,
};
pub use mesh_batch::{
    MeshBatchQueue, MeshBatcher, MeshDrawGroup, MeshVertex, SustainTrailParams,
    SUSTAIN_QUAD_INDICES,
};
pub use rain::RainPostPass;
pub use text::{
    format_hud_score, BitmapFont, FontAtlasImage, FontCase, FontError, Glyph, GlyphQuad,
    StackTextBuffer, TextAlign, TextStyle,
};
