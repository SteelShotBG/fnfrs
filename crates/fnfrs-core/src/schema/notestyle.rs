use serde::{Deserialize, Serialize};

use crate::schema::character::UnnamedAnimationData;
use crate::schema::{deserialize_non_empty_opt_string, deserialize_vec2_zero};

/// Note style JSON schema (`assets/data/notestyles/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleData {
    #[serde(default = "default_notestyle_version")]
    pub version: String,
    pub name: String,
    pub author: String,
    #[serde(default)]
    pub fallback: Option<String>,
    pub assets: NoteStyleAssetsData,
}

/// Asset sections inside a [`NoteStyleData`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleAssetsData {
    #[serde(default)]
    pub note: Option<NoteStyleAssetData<NoteStyleDataNote>>,
    #[serde(default)]
    pub hold_note: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub note_strumline: Option<NoteStyleAssetData<NoteStyleDataNoteStrumline>>,
    #[serde(default)]
    pub note_splash: Option<NoteStyleAssetData<NoteStyleDataNoteSplash>>,
    #[serde(default)]
    pub hold_note_cover: Option<NoteStyleAssetData<NoteStyleDataHoldNoteCover>>,
    #[serde(default)]
    pub countdown_three: Option<NoteStyleAssetData<NoteStyleDataCountdown>>,
    #[serde(default)]
    pub countdown_two: Option<NoteStyleAssetData<NoteStyleDataCountdown>>,
    #[serde(default)]
    pub countdown_one: Option<NoteStyleAssetData<NoteStyleDataCountdown>>,
    #[serde(default)]
    pub countdown_go: Option<NoteStyleAssetData<NoteStyleDataCountdown>>,
    #[serde(default)]
    pub judgement_sick: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub judgement_good: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub judgement_bad: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub judgement_shit: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number0: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number1: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number2: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number3: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number4: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number5: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number6: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number7: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number8: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
    #[serde(default)]
    pub combo_number9: Option<NoteStyleAssetData<NoteStyleDataEmpty>>,
}

/// Generic wrapper shared by every [`NoteStyleAssetsData`] entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleAssetData<T> {
    #[serde(default, deserialize_with = "deserialize_non_empty_opt_string")]
    pub asset_path: Option<String>,
    #[serde(default = "default_one")]
    pub scale: f32,
    #[serde(default, deserialize_with = "deserialize_vec2_zero")]
    pub offsets: [f32; 2],
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default = "default_one")]
    pub alpha: f32,
    #[serde(default)]
    pub animated: bool,
    #[serde(default = "default_none")]
    pub data: Option<T>,
}

/// Empty payload for holdNote, judgements, and combo digits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoteStyleDataEmpty {}

/// Note sprite animations for the 4 lanes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteStyleDataNote {
    pub left: UnnamedAnimationData,
    pub down: UnnamedAnimationData,
    pub up: UnnamedAnimationData,
    pub right: UnnamedAnimationData,
}

/// Countdown step audio reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleDataCountdown {
    pub audio_path: String,
}

/// Strumline receptor animations for each direction (`NoteStyleData_NoteStrumline`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleDataNoteStrumline {
    pub left_static: UnnamedAnimationData,
    pub left_press: UnnamedAnimationData,
    pub left_confirm: UnnamedAnimationData,
    pub left_confirm_hold: UnnamedAnimationData,
    pub down_static: UnnamedAnimationData,
    pub down_press: UnnamedAnimationData,
    pub down_confirm: UnnamedAnimationData,
    pub down_confirm_hold: UnnamedAnimationData,
    pub up_static: UnnamedAnimationData,
    pub up_press: UnnamedAnimationData,
    pub up_confirm: UnnamedAnimationData,
    pub up_confirm_hold: UnnamedAnimationData,
    pub right_static: UnnamedAnimationData,
    pub right_press: UnnamedAnimationData,
    pub right_confirm: UnnamedAnimationData,
    pub right_confirm_hold: UnnamedAnimationData,
}

/// Note splash animation variants and blend settings (`NoteStyleData_NoteSplash`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleDataNoteSplash {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_framerate")]
    pub framerate_default: u32,
    #[serde(default = "default_variance")]
    pub framerate_variance: u32,
    #[serde(default = "default_blend")]
    pub blend_mode: String,
    #[serde(default)]
    pub left_splashes: Vec<UnnamedAnimationData>,
    #[serde(default)]
    pub down_splashes: Vec<UnnamedAnimationData>,
    #[serde(default)]
    pub up_splashes: Vec<UnnamedAnimationData>,
    #[serde(default)]
    pub right_splashes: Vec<UnnamedAnimationData>,
}

/// Hold note cover configuration (`NoteStyleData_HoldNoteCover`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleDataHoldNoteCover {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub left: Option<NoteStyleDataHoldNoteCoverDirection>,
    #[serde(default)]
    pub down: Option<NoteStyleDataHoldNoteCoverDirection>,
    #[serde(default)]
    pub up: Option<NoteStyleDataHoldNoteCoverDirection>,
    #[serde(default)]
    pub right: Option<NoteStyleDataHoldNoteCoverDirection>,
}

/// Per-direction start/hold/end animations for hold covers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteStyleDataHoldNoteCoverDirection {
    #[serde(default, deserialize_with = "deserialize_non_empty_opt_string")]
    pub asset_path: Option<String>,
    #[serde(default)]
    pub start: Option<UnnamedAnimationData>,
    #[serde(default)]
    pub hold: Option<UnnamedAnimationData>,
    #[serde(default)]
    pub end: Option<UnnamedAnimationData>,
}

fn default_notestyle_version() -> String {
    "1.1.0".to_string()
}
fn default_one() -> f32 {
    1.0
}
fn default_true() -> bool {
    true
}
fn default_framerate() -> u32 {
    24
}
fn default_variance() -> u32 {
    2
}
fn default_blend() -> String {
    "normal".to_string()
}
fn default_none<T>() -> Option<T> {
    None
}
