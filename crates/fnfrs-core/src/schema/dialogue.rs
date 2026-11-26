//! Data schemas for Week 6 V-Slice dialogue conversations, boxes, and speaker portraits.

use serde::Deserialize;

use crate::schema::character::AnimationData;
use crate::schema::{deserialize_non_empty_opt_string, deserialize_vec2_zero};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueBackdropData {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub fade_time: f32,
    #[serde(default = "default_backdrop_color")]
    pub color: String,
}

fn default_backdrop_color() -> String {
    "#BFB3DFD8".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueMusicData {
    #[serde(default)]
    pub asset: String,
    #[serde(default)]
    pub fade_time: f32,
    #[serde(default = "default_true")]
    pub looped: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueOutroData {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub fade_time: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueLineData {
    #[serde(default)]
    pub speaker: String,
    #[serde(default = "default_talk_enter")]
    pub speaker_animation: String,
    #[serde(default = "default_box_name", rename = "box")]
    pub dialogue_box: String,
    #[serde(default = "default_box_enter")]
    pub box_animation: String,
    #[serde(default)]
    pub text: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub backdrop: Option<DialogueBackdropData>,
    #[serde(default)]
    pub music: Option<DialogueMusicData>,
    #[serde(default)]
    pub outro: Option<DialogueOutroData>,
    #[serde(default)]
    pub dialogue: Vec<DialogueLineData>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueTextStyleData {
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default = "default_text_width")]
    pub width: f32,
    #[serde(default = "default_text_color")]
    pub color: String,
    #[serde(default)]
    pub font_family: String,
    #[serde(default = "default_shadow_color")]
    pub shadow_color: String,
    #[serde(default)]
    pub shadow_width: f32,
}

impl Default for DialogueTextStyleData {
    fn default() -> Self {
        Self {
            offsets: [225.0, 450.0],
            width: 900.0,
            color: default_text_color(),
            font_family: String::new(),
            shadow_color: default_shadow_color(),
            shadow_width: 2.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueBoxData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub asset_path: String,
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub text: DialogueTextStyleData,
    #[serde(default)]
    pub animations: Vec<AnimationData>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogueSpeakerData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub asset_path: String,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default = "default_scale")]
    pub scale: f32,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default, deserialize_with = "deserialize_non_empty_opt_string")]
    pub music: Option<String>,
    #[serde(default)]
    pub animations: Vec<AnimationData>,
}

fn default_true() -> bool {
    true
}

fn default_talk_enter() -> String {
    "talkEnter".to_owned()
}

fn default_box_name() -> String {
    "default".to_owned()
}

fn default_box_enter() -> String {
    "enter".to_owned()
}

fn default_text_width() -> f32 {
    900.0
}

fn default_text_color() -> String {
    "#3F2021".to_owned()
}

fn default_shadow_color() -> String {
    "#D89494".to_owned()
}

fn default_scale() -> f32 {
    5.4
}
