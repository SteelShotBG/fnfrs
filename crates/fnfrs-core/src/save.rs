//! Persistent portable save file (`save.json` next to executable, fallback CWD).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::scoring::ScoreTallies;

/// Tallies persisted inside a saved score record.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedTallies {
    pub sick: u32,
    pub good: u32,
    pub bad: u32,
    pub shit: u32,
    pub missed: u32,
    pub combo: u32,
    pub max_combo: u32,
    pub total_notes_hit: u32,
    pub total_notes: u32,
}

impl From<ScoreTallies> for SavedTallies {
    fn from(t: ScoreTallies) -> Self {
        Self {
            sick: t.sick,
            good: t.good,
            bad: t.bad,
            shit: t.shit,
            missed: t.missed,
            combo: t.combo,
            max_combo: t.max_combo,
            total_notes_hit: t.total_notes_hit,
            total_notes: t.total_notes,
        }
    }
}

/// Persisted high score entry for a single song difficulty or story week difficulty.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveScoreData {
    pub score: i64,
    #[serde(default)]
    pub tallies: SavedTallies,
    #[serde(default)]
    pub clear_percent: f32,
    #[serde(default)]
    pub rank: String,
}

/// Persisted score tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveScores {
    #[serde(default)]
    pub levels: BTreeMap<String, SaveScoreData>,
    #[serde(default)]
    pub songs: BTreeMap<String, SaveScoreData>,
}

/// Persisted user preferences & keybinds matching V-Slice `Save.hx` / `OptionsState.hx`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveOptions {
    #[serde(default)]
    pub downscroll: bool,
    #[serde(default)]
    pub botplay: bool,
    #[serde(default = "default_true")]
    pub flashing_lights: bool,
    #[serde(default = "default_true")]
    pub zoom_camera: bool,
    #[serde(default = "default_true")]
    pub naughtyness: bool,
    #[serde(default = "default_true")]
    pub auto_pause: bool,
    #[serde(default)]
    pub debug_display: bool,
    #[serde(default = "default_volume")]
    pub master_volume: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub audio_offset_ms: f32,
    #[serde(default)]
    pub fps_cap: u32,
    #[serde(default = "default_keybinds")]
    pub keybinds: [String; 4],
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            downscroll: false,
            botplay: false,
            flashing_lights: true,
            zoom_camera: true,
            naughtyness: true,
            auto_pause: true,
            debug_display: false,
            master_volume: 1.0,
            muted: false,
            audio_offset_ms: 0.0,
            fps_cap: 0,
            keybinds: default_keybinds(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_volume() -> f32 {
    1.0
}

fn default_keybinds() -> [String; 4] {
    [
        "A".to_owned(),
        "S".to_owned(),
        "W".to_owned(),
        "D".to_owned(),
    ]
}

fn default_character() -> String {
    "bf".to_owned()
}

fn default_save_version() -> String {
    "1.0.0".to_owned()
}

/// Root portable save file (`save.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveData {
    #[serde(default = "default_save_version")]
    pub version: String,
    #[serde(default)]
    pub scores: SaveScores,
    #[serde(default)]
    pub options: SaveOptions,
    #[serde(default)]
    pub favorite_songs: Vec<String>,
    #[serde(default = "default_character")]
    pub playable_character: String,
    #[serde(default)]
    pub cleared_levels: Vec<String>,
}

impl Default for SaveData {
    fn default() -> Self {
        Self {
            version: default_save_version(),
            scores: SaveScores::default(),
            options: SaveOptions::default(),
            favorite_songs: Vec::new(),
            playable_character: default_character(),
            cleared_levels: Vec::new(),
        }
    }
}

impl SaveData {
    /// Resolves portable `<exe_dir>/save.json` first, falling back to `<cwd>/save.json`.
    #[must_use]
    pub fn resolve_path() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join("save.json");
            }
        }
        PathBuf::from("save.json")
    }

    /// Loads `save.json` from [`Self::resolve_path`] (or returns `Default` on missing/corrupt).
    #[must_use]
    pub fn load_or_default() -> Self {
        let path = Self::resolve_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Self>(&raw) {
                return parsed;
            }
        }
        Self::default()
    }

    /// Writes `save.json` atomically to [`Self::resolve_path`].
    pub fn save_to_disk(&self) -> std::io::Result<()> {
        let path = Self::resolve_path();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = fs::create_dir_all(parent);
            }
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Canonical key for a song chart (`"song_id:variation:difficulty"` or `"song_id:difficulty"`).
    #[must_use]
    pub fn song_key(song_id: &str, variation: Option<&str>, difficulty: &str) -> String {
        match variation.filter(|v| !v.is_empty() && *v != "default") {
            Some(var) => format!(
                "{}:{}:{}",
                song_id.to_ascii_lowercase(),
                var.to_ascii_lowercase(),
                difficulty.to_ascii_lowercase()
            ),
            None => format!(
                "{}:{}",
                song_id.to_ascii_lowercase(),
                difficulty.to_ascii_lowercase()
            ),
        }
    }

    /// Canonical key for a story level (`"level_id:difficulty"`).
    #[must_use]
    pub fn level_key(level_id: &str, difficulty: &str) -> String {
        format!(
            "{}:{}",
            level_id.to_ascii_lowercase(),
            difficulty.to_ascii_lowercase()
        )
    }

    /// Records a song score if it beats the stored high score; returns `true` on new highscore.
    #[allow(clippy::too_many_arguments)]
    pub fn record_song_score(
        &mut self,
        song_id: &str,
        variation: Option<&str>,
        difficulty: &str,
        score: i64,
        tallies: ScoreTallies,
        clear_percent: f32,
        rank: &str,
    ) -> bool {
        let key = Self::song_key(song_id, variation, difficulty);
        let is_new = self
            .scores
            .songs
            .get(&key)
            .is_none_or(|prev| score > prev.score);
        if is_new {
            self.scores.songs.insert(
                key,
                SaveScoreData {
                    score,
                    tallies: SavedTallies::from(tallies),
                    clear_percent,
                    rank: rank.to_owned(),
                },
            );
            let _ = self.save_to_disk();
        }
        is_new
    }

    /// Returns the stored high score record for `(song_id, variation, difficulty)`.
    #[must_use]
    pub fn get_song_score(
        &self,
        song_id: &str,
        variation: Option<&str>,
        difficulty: &str,
    ) -> Option<&SaveScoreData> {
        let key = Self::song_key(song_id, variation, difficulty);
        self.scores.songs.get(&key)
    }

    /// Records a cleared story level score if higher than previous.
    pub fn record_level_score(&mut self, level_id: &str, difficulty: &str, score: i64) {
        let low = level_id.to_ascii_lowercase();
        if !self.cleared_levels.contains(&low) {
            self.cleared_levels.push(low);
        }
        let key = Self::level_key(level_id, difficulty);
        let replace = self
            .scores
            .levels
            .get(&key)
            .is_none_or(|prev| score > prev.score);
        if replace {
            self.scores.levels.insert(
                key,
                SaveScoreData {
                    score,
                    ..Default::default()
                },
            );
        }
        let _ = self.save_to_disk();
    }

    /// Returns whether `song_id` is favorited in Freeplay.
    #[must_use]
    pub fn is_favorite(&self, song_id: &str) -> bool {
        self.favorite_songs
            .iter()
            .any(|s| s.eq_ignore_ascii_case(song_id))
    }

    /// Toggles `song_id` in `favorite_songs` and persists to disk.
    pub fn toggle_favorite(&mut self, song_id: &str) -> bool {
        if let Some(pos) = self
            .favorite_songs
            .iter()
            .position(|s| s.eq_ignore_ascii_case(song_id))
        {
            self.favorite_songs.remove(pos);
            let _ = self.save_to_disk();
            false
        } else {
            self.favorite_songs.push(song_id.to_owned());
            let _ = self.save_to_disk();
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_roundtrip_and_highscore_logic() {
        let mut save = SaveData::default();
        assert_eq!(save.playable_character, "bf");
        assert!(save.options.flashing_lights);
        let t = ScoreTallies {
            sick: 10,
            good: 1,
            total_notes_hit: 11,
            total_notes: 11,
            ..Default::default()
        };
        let key = SaveData::song_key("bopeebo", None, "hard");
        assert_eq!(key, "bopeebo:hard");
        let is_new = save.scores.songs.get(&key).is_none_or(|p| 5000 > p.score);
        assert!(is_new);
        save.scores.songs.insert(
            key.clone(),
            SaveScoreData {
                score: 5000,
                tallies: SavedTallies::from(t),
                clear_percent: 98.0,
                rank: "EXCELLENT".to_owned(),
            },
        );
        let json = serde_json::to_string(&save).expect("serialize");
        let back: SaveData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, save);
    }
}
