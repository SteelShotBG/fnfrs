use serde::{Deserialize, Serialize};

use crate::schema::character::AnimationData;
use crate::schema::deserialize_vec2_zero;

/// Story mode level JSON schema (`assets/data/levels/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelData {
    #[serde(default = "default_level_version")]
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub capsule: Option<LevelCapsuleData>,
    pub title_asset: String,
    #[serde(default)]
    pub props: Vec<LevelPropData>,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_songs")]
    pub songs: Vec<String>,
    #[serde(default = "default_background")]
    pub background: String,
}

/// Freeplay capsule label override (`LevelCapsuleData`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelCapsuleData {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
}

/// Animated prop rendered on the story menu week banner (`LevelPropData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelPropData {
    pub asset_path: String,
    #[serde(default = "default_one")]
    pub scale: f32,
    #[serde(default = "default_one")]
    pub alpha: f32,
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default = "default_one")]
    pub dance_every: f32,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default)]
    pub animations: Vec<AnimationData>,
    #[serde(default)]
    pub starting_animation: String,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
}

fn default_level_version() -> String {
    "1.0.0".to_string()
}
fn default_true() -> bool {
    true
}
fn default_one() -> f32 {
    1.0
}
fn default_songs() -> Vec<String> {
    vec!["bopeebo".to_string()]
}
fn default_background() -> String {
    "#F9CF51".to_string()
}
