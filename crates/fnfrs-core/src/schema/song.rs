use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize};

use crate::constants::{
    DEFAULT_BPM, DEFAULT_NOTE_STYLE, DEFAULT_STAGE, DEFAULT_TIME_SIGNATURE_DEN,
    DEFAULT_TIME_SIGNATURE_NUM,
};
use crate::schema::deserialize_non_empty_opt_string;

/// Song metadata (`<song>-metadata[-<variation>].json`), matching `SongMetadata` in `SongData.hx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongMetadata {
    #[serde(default = "default_metadata_version")]
    pub version: String,
    #[serde(default = "default_unknown")]
    pub song_name: String,
    #[serde(default = "default_unknown")]
    pub artist: String,
    #[serde(default)]
    pub charter: Option<String>,
    #[serde(default)]
    pub divisions: Option<u32>,
    #[serde(default)]
    pub looped: bool,
    #[serde(default)]
    pub offsets: SongOffsets,
    #[serde(default)]
    pub play_data: SongPlayData,
    #[serde(default)]
    pub generated_by: String,
    #[serde(default)]
    pub time_format: SongTimeFormat,
    #[serde(default = "default_time_changes")]
    pub time_changes: Vec<SongTimeChange>,
}

/// Timestamp unit format inside song metadata/charts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SongTimeFormat {
    Ticks,
    Float,
    #[default]
    Ms,
}

/// Tempo and time-signature marker (`SongTimeChange`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongTimeChange {
    #[serde(rename = "t")]
    pub time_stamp: f64,
    #[serde(rename = "b", default)]
    pub beat_time: f64,
    #[serde(rename = "bpm", default)]
    pub bpm: f64,
    #[serde(rename = "n", default = "default_time_sig_num")]
    pub time_signature_num: u32,
    #[serde(rename = "d", default = "default_time_sig_den")]
    pub time_signature_den: u32,
    #[serde(
        rename = "bt",
        default = "default_beat_tuplets",
        deserialize_with = "deserialize_beat_tuplets"
    )]
    pub beat_tuplets: Vec<u32>,
}

impl Default for SongTimeChange {
    fn default() -> Self {
        Self {
            time_stamp: 0.0,
            beat_time: 0.0,
            bpm: DEFAULT_BPM,
            time_signature_num: DEFAULT_TIME_SIGNATURE_NUM,
            time_signature_den: DEFAULT_TIME_SIGNATURE_DEN,
            beat_tuplets: default_beat_tuplets(),
        }
    }
}

/// Instrumental and vocal audio offsets in milliseconds (`SongOffsets`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongOffsets {
    #[serde(default)]
    pub instrumental: f64,
    #[serde(default)]
    pub alt_instrumentals: FxHashMap<String, f64>,
    #[serde(default)]
    pub vocals: FxHashMap<String, f64>,
    #[serde(default)]
    pub alt_vocals: FxHashMap<String, FxHashMap<String, f64>>,
}

impl SongOffsets {
    /// Returns the instrumental offset in ms for the given alt instrumental (or base).
    #[must_use]
    pub fn get_instrumental_offset(&self, instrumental: Option<&str>) -> f64 {
        match instrumental {
            Some(inst) if !inst.is_empty() => self
                .alt_instrumentals
                .get(inst)
                .copied()
                .unwrap_or(self.instrumental),
            _ => self.instrumental,
        }
    }

    /// Returns the vocal offset in ms for `char_id` relative to the given instrumental.
    #[must_use]
    pub fn get_vocal_offset(&self, char_id: &str, instrumental: Option<&str>) -> f64 {
        match instrumental {
            Some(inst) if !inst.is_empty() => self
                .alt_vocals
                .get(inst)
                .and_then(|map| map.get(char_id))
                .copied()
                .unwrap_or(0.0),
            _ => self.vocals.get(char_id).copied().unwrap_or(0.0),
        }
    }
}

/// Gameplay configuration embedded in `SongMetadata` (`SongPlayData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongPlayData {
    #[serde(default)]
    pub song_variations: Vec<String>,
    #[serde(default)]
    pub difficulties: Vec<String>,
    #[serde(default)]
    pub characters: SongCharacterData,
    #[serde(default = "default_stage")]
    pub stage: String,
    #[serde(default = "default_note_style")]
    pub note_style: String,
    #[serde(default)]
    pub ratings: FxHashMap<String, i32>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub sticker_pack: Option<String>,
    #[serde(default)]
    pub preview_start: f64,
    #[serde(default = "default_preview_end")]
    pub preview_end: f64,
}

impl Default for SongPlayData {
    fn default() -> Self {
        Self {
            song_variations: Vec::new(),
            difficulties: Vec::new(),
            characters: SongCharacterData::default(),
            stage: default_stage(),
            note_style: default_note_style(),
            ratings: FxHashMap::default(),
            album: None,
            sticker_pack: None,
            preview_start: 0.0,
            preview_end: default_preview_end(),
        }
    }
}

/// Character and audio-track routing for a song variation (`SongCharacterData`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongCharacterData {
    #[serde(default)]
    pub player: String,
    #[serde(default)]
    pub girlfriend: String,
    #[serde(default)]
    pub opponent: String,
    #[serde(default)]
    pub instrumental: String,
    #[serde(default)]
    pub alt_instrumentals: Vec<String>,
    #[serde(default)]
    pub opponent_vocals: Option<Vec<String>>,
    #[serde(default)]
    pub player_vocals: Option<Vec<String>>,
}

/// Song chart file (`<song>-chart[-<variation>].json`), matching `SongChartData`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongChartData {
    #[serde(default = "default_chart_version")]
    pub version: String,
    #[serde(default)]
    pub scroll_speed: FxHashMap<String, f32>,
    #[serde(default)]
    pub events: Vec<SongEventData>,
    #[serde(default)]
    pub notes: FxHashMap<String, Vec<SongNoteData>>,
    #[serde(default)]
    pub generated_by: String,
}

impl SongChartData {
    /// Returns the scroll speed for `difficulty`, falling back to `"default"` then `1.0`.
    #[must_use]
    pub fn get_scroll_speed(&self, difficulty: &str) -> f32 {
        let speed = self
            .scroll_speed
            .get(difficulty)
            .copied()
            .filter(|&v| v != 0.0)
            .or_else(|| {
                if difficulty != "default" {
                    self.scroll_speed
                        .get("default")
                        .copied()
                        .filter(|&v| v != 0.0)
                } else {
                    None
                }
            });
        speed.unwrap_or(1.0)
    }

    /// Returns the note list for `difficulty`, falling back to `"normal"` then empty slice.
    #[must_use]
    pub fn get_notes(&self, difficulty: &str) -> &[SongNoteData] {
        if let Some(notes) = self.notes.get(difficulty) {
            return notes;
        }
        if difficulty != "normal" {
            if let Some(notes) = self.notes.get("normal") {
                return notes;
            }
        }
        &[]
    }
}

/// Chart event entry (`SongEventData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongEventData {
    #[serde(rename = "t")]
    pub time: f64,
    #[serde(rename = "e")]
    pub event_kind: String,
    #[serde(rename = "v", default)]
    pub value: serde_json::Value,
}

impl SongEventData {
    /// Returns the event `value` as a scalar integer (e.g. `FocusCamera` `v: 1`).
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        self.value.as_i64()
    }

    /// Returns the event `value` as a scalar float (e.g. `ZoomCamera` `v: 1.3`).
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        self.value.as_f64()
    }

    /// Retrieves an integer field by key from an object `value`.
    #[must_use]
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        let field = self.value.get(key)?;
        field
            .as_i64()
            .or_else(|| field.as_str().and_then(|s| s.parse::<i64>().ok()))
    }

    /// Retrieves a float field by key from an object `value`.
    #[must_use]
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        let field = self.value.get(key)?;
        field
            .as_f64()
            .or_else(|| field.as_str().and_then(|s| s.parse::<f64>().ok()))
    }

    /// Retrieves a string field by key.
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.value.get(key).and_then(|v| v.as_str())
    }

    /// Retrieves a bool field by key.
    #[must_use]
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.value.get(key).and_then(|v| v.as_bool())
    }
}

/// Single note entry (`SongNoteData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongNoteData {
    #[serde(rename = "t")]
    pub time: f64,
    #[serde(rename = "d")]
    pub data: i32,
    #[serde(rename = "l", default)]
    pub length: f64,
    #[serde(
        rename = "k",
        default,
        deserialize_with = "deserialize_non_empty_opt_string"
    )]
    pub kind: Option<String>,
    #[serde(rename = "p", default)]
    pub params: Vec<NoteParamData>,
}

impl SongNoteData {
    /// Lane direction index (`0 = Left, 1 = Down, 2 = Up, 3 = Right`).
    #[inline]
    #[must_use]
    pub fn direction(&self, strumline_size: usize) -> usize {
        self.data.rem_euclid(strumline_size as i32) as usize
    }

    /// Strumline owner (`0 = Player, 1 = Opponent`).
    #[inline]
    #[must_use]
    pub fn strumline_index(&self, strumline_size: usize) -> i32 {
        self.data.div_euclid(strumline_size as i32)
    }

    /// Returns `true` if this note belongs to the player's strumline (`strumline_index == 0`).
    #[inline]
    #[must_use]
    pub fn is_player_note(&self, strumline_size: usize) -> bool {
        self.strumline_index(strumline_size) == 0
    }

    /// Returns `true` if this note has a sustain tail (`length > 0.0`).
    #[inline]
    #[must_use]
    pub fn is_hold_note(&self) -> bool {
        self.length > 0.0
    }
}

/// Custom parameter attached to a note (`NoteParamData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteParamData {
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "v", default)]
    pub value: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BeatTupletsRaw {
    Array(Vec<u32>),
    Single(u32),
}

fn deserialize_beat_tuplets<'de, D>(deserializer: D) -> Result<Vec<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<BeatTupletsRaw>::deserialize(deserializer)?;
    Ok(match raw {
        Some(BeatTupletsRaw::Array(v)) if !v.is_empty() => v,
        Some(BeatTupletsRaw::Single(n)) => vec![n],
        _ => default_beat_tuplets(),
    })
}

fn default_metadata_version() -> String {
    "2.2.4".to_string()
}
fn default_chart_version() -> String {
    "2.0.0".to_string()
}
fn default_unknown() -> String {
    "Unknown".to_string()
}
fn default_stage() -> String {
    DEFAULT_STAGE.to_string()
}
fn default_note_style() -> String {
    DEFAULT_NOTE_STYLE.to_string()
}
fn default_preview_end() -> f64 {
    0.2
}
fn default_time_sig_num() -> u32 {
    DEFAULT_TIME_SIGNATURE_NUM
}
fn default_time_sig_den() -> u32 {
    DEFAULT_TIME_SIGNATURE_DEN
}
fn default_beat_tuplets() -> Vec<u32> {
    vec![4, 4, 4, 4]
}
fn default_time_changes() -> Vec<SongTimeChange> {
    vec![SongTimeChange::default()]
}
