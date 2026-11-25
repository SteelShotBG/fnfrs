use serde::{Deserialize, Serialize};

use crate::schema::stage::TextureAtlasData;
use crate::schema::{deserialize_non_empty_opt_string, deserialize_vec2_zero};

/// Character animation definition (`AnimationData` in `AnimationData.hx`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationData {
    pub name: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_empty_opt_string")]
    pub asset_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default)]
    pub looped: bool,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default = "default_frame_rate")]
    pub frame_rate: u32,
    #[serde(default)]
    pub frame_indices: Vec<usize>,
    #[serde(default = "default_anim_type")]
    pub anim_type: String,
    #[serde(default)]
    pub render_type: String,
    #[serde(default)]
    pub atlas_settings: Option<TextureAtlasData>,
}

/// Unnamed animation definition used in note styles (`UnnamedAnimationData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnnamedAnimationData {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_empty_opt_string")]
    pub asset_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default)]
    pub looped: bool,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default = "default_frame_rate")]
    pub frame_rate: u32,
    #[serde(default)]
    pub frame_indices: Vec<usize>,
    #[serde(default = "default_anim_type")]
    pub anim_type: String,
    #[serde(default)]
    pub render_type: String,
    #[serde(default)]
    pub atlas_settings: Option<TextureAtlasData>,
}

impl UnnamedAnimationData {
    /// Converts into a named [`AnimationData`].
    #[must_use]
    pub fn to_named(&self, name: impl Into<String>) -> AnimationData {
        AnimationData {
            name: name.into(),
            prefix: self.prefix.clone(),
            asset_path: self.asset_path.clone(),
            offsets: self.offsets,
            looped: self.looped,
            flip_x: self.flip_x,
            flip_y: self.flip_y,
            frame_rate: self.frame_rate,
            frame_indices: self.frame_indices.clone(),
            anim_type: self.anim_type.clone(),
            render_type: self.render_type.clone(),
            atlas_settings: self.atlas_settings.clone(),
        }
    }
}

/// Character rendering backend (`CharacterRenderType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CharacterRenderType {
    #[default]
    Sparrow,
    Packer,
    #[serde(rename = "multisparrow")]
    MultiSparrow,
    #[serde(rename = "animateatlas")]
    AnimateAtlas,
    #[serde(rename = "multianimateatlas")]
    MultiAnimateAtlas,
    Custom,
}

/// Character JSON schema (`assets/data/characters/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterData {
    #[serde(default = "default_char_version")]
    pub version: String,
    #[serde(default = "default_char_name")]
    pub name: String,
    #[serde(default)]
    pub render_type: CharacterRenderType,
    pub asset_path: String,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub health_icon: Option<HealthIconData>,
    #[serde(default)]
    pub death: Option<DeathData>,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub camera_offsets: [f32; 2],
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default = "default_dance_every")]
    pub dance_every: f32,
    #[serde(default = "default_sing_time")]
    pub sing_time: f32,
    #[serde(default)]
    pub animations: Vec<AnimationData>,
    #[serde(default = "default_starting_anim")]
    pub starting_animation: String,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub apply_stage_matrix: bool,
    #[serde(default)]
    pub atlas_settings: Option<TextureAtlasData>,
}

/// Health icon configuration (`HealthIconData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthIconData {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "default_true")]
    pub should_bop: bool,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub is_pixel: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
}

/// Death camera & delay overrides (`DeathData`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeathData {
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub camera_offsets: [f32; 2],
    #[serde(default = "default_scale")]
    pub camera_zoom: f32,
    #[serde(default)]
    pub pre_transition_delay: f32,
}

fn default_frame_rate() -> u32 {
    24
}
fn default_anim_type() -> String {
    "framelabel".to_string()
}
fn default_char_version() -> String {
    "1.0.2".to_string()
}
fn default_char_name() -> String {
    "Untitled Character".to_string()
}
fn default_scale() -> f32 {
    1.0
}
fn default_dance_every() -> f32 {
    1.0
}
fn default_sing_time() -> f32 {
    8.0
}
fn default_starting_anim() -> String {
    "idle".to_string()
}
fn default_true() -> bool {
    true
}
