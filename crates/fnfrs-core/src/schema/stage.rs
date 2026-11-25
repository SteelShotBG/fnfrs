use serde::{Deserialize, Serialize};

use crate::schema::character::AnimationData;
use crate::schema::{deserialize_vec2_one, deserialize_vec2_zero};

/// Stage JSON schema (`assets/data/stages/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageData {
    #[serde(default = "default_stage_version")]
    pub version: String,
    #[serde(default = "default_unknown")]
    pub name: String,
    #[serde(default)]
    pub props: Vec<StageDataProp>,
    #[serde(default)]
    pub characters: StageDataCharacters,
    #[serde(default = "default_one")]
    pub camera_zoom: f32,
    #[serde(default = "default_shared")]
    pub directory: String,
}

/// Character placement blocks inside a stage (`StageDataCharacters`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDataCharacters {
    pub bf: StageDataCharacter,
    pub dad: StageDataCharacter,
    pub gf: StageDataCharacter,
}

impl Default for StageDataCharacters {
    fn default() -> Self {
        Self {
            bf: StageDataCharacter {
                camera_offsets: [-100.0, -100.0],
                ..Default::default()
            },
            dad: StageDataCharacter {
                camera_offsets: [100.0, -100.0],
                ..Default::default()
            },
            gf: StageDataCharacter::default(),
        }
    }
}

/// Single character slot placement inside [`StageDataCharacters`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageDataCharacter {
    #[serde(default)]
    pub z_index: i32,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub position: [f32; 2],
    #[serde(default = "default_one")]
    pub scale: f32,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub camera_offsets: [f32; 2],
    #[serde(
        default = "default_vec2_one",
        deserialize_with = "deserialize_vec2_one"
    )]
    pub scroll: [f32; 2],
    #[serde(default = "default_one")]
    pub alpha: f32,
    #[serde(default)]
    pub angle: f32,
}

impl Default for StageDataCharacter {
    fn default() -> Self {
        Self {
            z_index: 0,
            position: [0.0, 0.0],
            scale: 1.0,
            camera_offsets: [0.0, 0.0],
            scroll: [1.0, 1.0],
            alpha: 1.0,
            angle: 0.0,
        }
    }
}

/// Scalar or `[x, y]` scale value used by stage props.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScaleSpec {
    Scalar(f32),
    Pair([f32; 2]),
}

impl Default for ScaleSpec {
    fn default() -> Self {
        Self::Scalar(1.0)
    }
}

impl ScaleSpec {
    /// Returns `[scale_x, scale_y]`.
    #[inline]
    #[must_use]
    pub fn as_xy(self) -> [f32; 2] {
        match self {
            Self::Scalar(v) => [v, v],
            Self::Pair(xy) => xy,
        }
    }
}

/// Stage prop JSON definition (`StageDataProp`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageDataProp {
    #[serde(default)]
    pub name: Option<String>,
    pub asset_path: String,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub position: [f32; 2],
    #[serde(default)]
    pub z_index: i32,
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default)]
    pub scale: ScaleSpec,
    #[serde(default = "default_one")]
    pub alpha: f32,
    #[serde(default)]
    pub dance_every: f32,
    #[serde(
        default = "default_vec2_one",
        deserialize_with = "deserialize_vec2_one"
    )]
    pub scroll: [f32; 2],
    #[serde(default)]
    pub animations: Vec<AnimationData>,
    #[serde(default)]
    pub starting_animation: Option<String>,
    #[serde(default = "default_sparrow")]
    pub anim_type: String,
    #[serde(default)]
    pub angle: f32,
    #[serde(default)]
    pub blend: String,
    #[serde(default = "default_white")]
    pub color: String,
    #[serde(default)]
    pub atlas_settings: Option<TextureAtlasData>,
}

/// Adobe Animate texture atlas options (`TextureAtlasData`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureAtlasData {
    #[serde(default)]
    pub swf_mode: bool,
    #[serde(default)]
    pub cache_on_load: bool,
    #[serde(default = "default_filter_quality")]
    pub filter_quality: i32,
    #[serde(default)]
    pub apply_stage_matrix: bool,
    #[serde(default)]
    pub use_render_texture: bool,
}

fn default_stage_version() -> String {
    "1.0.0".to_string()
}
fn default_unknown() -> String {
    "Unknown".to_string()
}
fn default_one() -> f32 {
    1.0
}
fn default_vec2_one() -> [f32; 2] {
    [1.0, 1.0]
}
fn default_shared() -> String {
    "shared".to_string()
}
fn default_sparrow() -> String {
    "sparrow".to_string()
}
fn default_white() -> String {
    "#FFFFFF".to_string()
}
fn default_filter_quality() -> i32 {
    1
}
