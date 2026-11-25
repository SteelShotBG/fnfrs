use std::fs;
use std::path::{Path, PathBuf};

use crate::constants::DEFAULT_VARIATION;
use crate::schema::song::SongMetadata;

/// Runtime asset locator mirroring `funkin/Paths.hx`.
///
/// Resolves `assets/` relative to the executable directory first, then the
/// current working directory. Never hardcodes machine-specific user paths.
#[derive(Debug, Clone)]
pub struct AssetLocator {
    root: PathBuf,
    current_level: Option<String>,
}

impl AssetLocator {
    /// Creates an [`AssetLocator`] pointing to an explicit `assets/` root directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            current_level: None,
        }
    }

    /// Discovers the `assets/` directory by checking `<exe_dir>/assets` first,
    /// then `<cwd>/assets`. Returns `None` if neither exists.
    #[must_use]
    pub fn discover() -> Option<Self> {
        discover_assets_root().map(Self::new)
    }

    /// Discovers the `assets/` directory or falls back to relative `"assets"`.
    #[must_use]
    pub fn discover_or_default() -> Self {
        Self::new(discover_assets_root().unwrap_or_else(|| PathBuf::from("assets")))
    }

    /// Returns the root `assets/` path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Sets or clears the active stage/level library (e.g. `"week6"`).
    pub fn set_current_level(&mut self, level: Option<&str>) {
        self.current_level = level.filter(|s| !s.is_empty()).map(str::to_ascii_lowercase);
    }

    /// Returns the active stage/level library, if any.
    #[must_use]
    pub fn current_level(&self) -> Option<&str> {
        self.current_level.as_deref()
    }

    /// Splits a `library:path` reference into `(Option<library>, stripped_path)`.
    #[must_use]
    pub fn split_library(raw: &str) -> (Option<&str>, &str) {
        match raw.split_once(':') {
            Some((lib, rest)) if !lib.is_empty() => (Some(lib), rest),
            Some((_, rest)) => (None, rest),
            None => (None, raw),
        }
    }

    /// Resolves a relative file path (already including subdirectory/extension, e.g.
    /// `"images/notes.png"`) using an optional explicit library or V-Slice fallback order:
    /// `current_level` -> `shared` -> `root`.
    #[must_use]
    pub fn resolve_file(&self, relative: &str, library: Option<&str>) -> PathBuf {
        let clean = relative.trim_start_matches(['/', '\\']);
        if let Some(lib) = library {
            return self.library_path(clean, lib);
        }

        if let Some(level) = &self.current_level {
            let level_candidate = self.library_path(clean, level);
            if level_candidate.exists() {
                return level_candidate;
            }
        }

        let shared_candidate = self.library_path(clean, "shared");
        if shared_candidate.exists() {
            return shared_candidate;
        }

        self.root.join(clean)
    }

    /// Resolves an asset key that may carry a `library:` prefix (e.g. `"week6:weeb/pixelUI/arrows-pixels"`).
    #[must_use]
    pub fn resolve_key(&self, folder: &str, key_with_lib: &str, ext: Option<&str>) -> PathBuf {
        let (lib, key) = Self::split_library(key_with_lib);
        let clean_key = key.trim_start_matches(['/', '\\']);
        let rel = match ext {
            Some(e) if !e.is_empty() => format!("{folder}/{clean_key}.{e}"),
            _ => format!("{folder}/{clean_key}"),
        };
        self.resolve_file(&rel, lib)
    }

    /// Resolves `<library>/file`, mapping `"preload"` and `"default"` to `<root>/file`.
    #[must_use]
    pub fn library_path(&self, file: &str, library: &str) -> PathBuf {
        let clean = file.trim_start_matches(['/', '\\']);
        if library.eq_ignore_ascii_case("preload") || library.eq_ignore_ascii_case("default") {
            self.root.join(clean)
        } else {
            self.root.join(library.to_ascii_lowercase()).join(clean)
        }
    }

    /// Resolves an image PNG path (`images/<key>.png`).
    #[must_use]
    pub fn image(&self, key: &str) -> PathBuf {
        self.resolve_key("images", key, Some("png"))
    }

    /// Resolves a Sparrow XML atlas path (`images/<key>.xml`).
    #[must_use]
    pub fn sparrow_xml(&self, key: &str) -> PathBuf {
        self.resolve_key("images", key, Some("xml"))
    }

    /// Resolves a Packer TXT atlas path (`images/<key>.txt`).
    #[must_use]
    pub fn packer_txt(&self, key: &str) -> PathBuf {
        self.resolve_key("images", key, Some("txt"))
    }

    /// Resolves an Adobe Animate texture atlas directory (`images/<key>`).
    #[must_use]
    pub fn animate_atlas_dir(&self, key: &str) -> PathBuf {
        self.resolve_key("images", key, None)
    }

    /// Resolves a sound effect `.ogg` path (`sounds/<key>.ogg`).
    #[must_use]
    pub fn sound(&self, key: &str) -> PathBuf {
        self.resolve_key("sounds", key, Some("ogg"))
    }

    /// Resolves a music `.ogg` path (`music/<key>.ogg`).
    #[must_use]
    pub fn music(&self, key: &str) -> PathBuf {
        self.resolve_key("music", key, Some("ogg"))
    }

    /// Resolves a font file (`fonts/<key>`).
    #[must_use]
    pub fn font(&self, key: &str) -> PathBuf {
        self.root
            .join("fonts")
            .join(key.trim_start_matches(['/', '\\']))
    }

    /// Resolves a JSON data file (`data/<key>.json`).
    #[must_use]
    pub fn data_json(&self, key: &str) -> PathBuf {
        self.resolve_key("data", key, Some("json"))
    }

    /// Resolves `data/songs/<song_id>/<song_id>-metadata[-<variation>].json`.
    #[must_use]
    pub fn song_metadata(&self, song_id: &str, variation: Option<&str>) -> PathBuf {
        let id = song_id.to_ascii_lowercase();
        let suffix = variation_suffix(variation);
        self.root
            .join("data")
            .join("songs")
            .join(&id)
            .join(format!("{id}-metadata{suffix}.json"))
    }

    /// Resolves `data/songs/<song_id>/<song_id>-chart[-<variation>].json`.
    #[must_use]
    pub fn song_chart(&self, song_id: &str, variation: Option<&str>) -> PathBuf {
        let id = song_id.to_ascii_lowercase();
        let suffix = variation_suffix(variation);
        self.root
            .join("data")
            .join("songs")
            .join(&id)
            .join(format!("{id}-chart{suffix}.json"))
    }

    /// Resolves `songs/<song_id>/Inst<suffix>.ogg`.
    #[must_use]
    pub fn song_inst(&self, song_id: &str, suffix: &str) -> PathBuf {
        let id = song_id.to_ascii_lowercase();
        self.root
            .join("songs")
            .join(id)
            .join(format!("Inst{suffix}.ogg"))
    }

    /// Resolves `songs/<song_id>/Voices<suffix>.ogg`.
    #[must_use]
    pub fn song_voices(&self, song_id: &str, suffix: &str) -> PathBuf {
        let id = song_id.to_ascii_lowercase();
        self.root
            .join("songs")
            .join(id)
            .join(format!("Voices{suffix}.ogg"))
    }

    /// Resolves the effective song variation for `(song_id, difficulty, variation)`,
    /// mirroring V-Slice `Paths.hx` / `PlayState.hx`:
    /// - Explicit non-default variation is used directly when its metadata exists
    ///   (and `"bf"` falls back to base `None` if no `-metadata-bf.json` exists).
    /// - When no variation is supplied and `difficulty` is `"erect"` or `"nightmare"`,
    ///   auto-selects `Some("erect")` if the base metadata is missing or lacks that
    ///   difficulty and `<song_id>-metadata-erect.json` exists.
    #[must_use]
    pub fn resolve_song_variation(
        &self,
        song_id: &str,
        difficulty: &str,
        variation: Option<&str>,
    ) -> Option<String> {
        let normalized_var = variation
            .map(str::trim)
            .filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case(DEFAULT_VARIATION))
            .map(str::to_ascii_lowercase);

        if let Some(var) = normalized_var {
            if self.song_metadata(song_id, Some(&var)).exists() {
                return Some(var);
            }
            if var == "bf" {
                return None;
            }
            return Some(var);
        }

        if difficulty.eq_ignore_ascii_case("erect") || difficulty.eq_ignore_ascii_case("nightmare") {
            let base_meta_path = self.song_metadata(song_id, None);
            let base_has_diff = fs::read_to_string(&base_meta_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<SongMetadata>(&raw).ok())
                .is_some_and(|meta| {
                    meta.play_data
                        .difficulties
                        .iter()
                        .any(|d| d.eq_ignore_ascii_case(difficulty))
                });
            if !base_has_diff && self.song_metadata(song_id, Some("erect")).exists() {
                return Some("erect".to_owned());
            }
        }

        None
    }

    /// Resolves the instrumental `.ogg` path for a song using `metadata.play_data.characters.instrumental`,
    /// falling back to `variation` and then the base `Inst.ogg`.
    #[must_use]
    pub fn resolve_song_inst(
        &self,
        song_id: &str,
        metadata: Option<&SongMetadata>,
        variation: Option<&str>,
    ) -> PathBuf {
        if let Some(inst) = metadata
            .map(|m| m.play_data.characters.instrumental.trim())
            .filter(|s| !s.is_empty())
        {
            let candidate = self.song_inst(song_id, &format!("-{inst}"));
            if candidate.exists() {
                return candidate;
            }
        }

        let var_suffix = variation_suffix(variation);
        if !var_suffix.is_empty() {
            let candidate = self.song_inst(song_id, &var_suffix);
            if candidate.exists() {
                return candidate;
            }
        }

        self.song_inst(song_id, "")
    }

    /// Resolves a character vocal `.ogg` path mirroring `Song.buildPlayerVoiceList` /
    /// `Song.buildOpponentVoiceList`: tries explicit vocal IDs first, then iteratively
    /// strips hyphenated suffixes (`pico-holding-nene` -> `pico-holding` -> `pico`),
    /// preferring `-<variation>` before falling back to base `Voices-<char>.ogg`.
    #[must_use]
    pub fn resolve_song_voices_for(
        &self,
        song_id: &str,
        explicit_ids: Option<&[String]>,
        fallback_char: &str,
        variation: Option<&str>,
    ) -> Option<PathBuf> {
        let var_suffix = variation_suffix(variation);

        // Pass 1: try explicit IDs as given (with variation suffix, then base).
        if let Some(ids) = explicit_ids.filter(|list| !list.is_empty()) {
            if !var_suffix.is_empty() {
                for id in ids {
                    let p = self.song_voices(song_id, &format!("-{id}{var_suffix}"));
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
            for id in ids {
                let p = self.song_voices(song_id, &format!("-{id}"));
                if p.exists() {
                    return Some(p);
                }
            }
        }

        // Pass 2: strip hyphenated character suffixes iteratively (e.g. `bf-pixel` -> `bf`).
        let fallback_slice = if fallback_char.trim().is_empty() {
            &[][..]
        } else {
            std::slice::from_ref(&fallback_char)
        };
        let explicit_iter = explicit_ids
            .into_iter()
            .flatten()
            .map(String::as_str)
            .chain(fallback_slice.iter().copied());

        if !var_suffix.is_empty() {
            for candidate in explicit_iter.clone() {
                if let Some(p) = self.find_voice_stripping_suffixes(song_id, candidate, &var_suffix)
                {
                    return Some(p);
                }
            }
        }

        for candidate in explicit_iter {
            if let Some(p) = self.find_voice_stripping_suffixes(song_id, candidate, "") {
                return Some(p);
            }
        }

        None
    }

    /// Legacy fallback for songs with an unsplit `Voices[-<variation>].ogg` stem.
    #[must_use]
    pub fn resolve_song_voices_fallback(
        &self,
        song_id: &str,
        variation: Option<&str>,
    ) -> Option<PathBuf> {
        let var_suffix = variation_suffix(variation);
        if !var_suffix.is_empty() {
            let with_var = self.song_voices(song_id, &var_suffix);
            if with_var.exists() {
                return Some(with_var);
            }
        }
        let base = self.song_voices(song_id, "");
        base.exists().then_some(base)
    }

    fn find_voice_stripping_suffixes(
        &self,
        song_id: &str,
        char_id: &str,
        var_suffix: &str,
    ) -> Option<PathBuf> {
        let mut current = char_id.trim();
        while !current.is_empty() {
            let p = self.song_voices(song_id, &format!("-{current}{var_suffix}"));
            if p.exists() {
                return Some(p);
            }
            match current.rfind('-') {
                Some(idx) => current = &current[..idx],
                None => break,
            }
        }
        None
    }
}

fn variation_suffix(variation: Option<&str>) -> String {
    match variation {
        Some(v) if !v.is_empty() && v != DEFAULT_VARIATION => format!("-{v}"),
        _ => String::new(),
    }
}

/// Checks `<exe_dir>/assets` first, then `<cwd>/assets`.
#[must_use]
pub fn discover_assets_root() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok();
    if let Some(exe_dir) = exe_path.as_deref().and_then(Path::parent) {
        let candidate = exe_dir.join("assets");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("assets");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("fnfrs_test_{tag}_{nanos}"));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn splits_library_prefix() {
        assert_eq!(
            AssetLocator::split_library("shared:notes"),
            (Some("shared"), "notes")
        );
        assert_eq!(
            AssetLocator::split_library("week6:weeb/pixelUI/arrows-pixels"),
            (Some("week6"), "weeb/pixelUI/arrows-pixels")
        );
        assert_eq!(
            AssetLocator::split_library("stageback"),
            (None, "stageback")
        );
    }

    #[test]
    fn resolves_level_shared_and_default_fallbacks() {
        let tmp = TempDir::new("paths");
        let week6_img = tmp.path.join("week6").join("images").join("weeb");
        let shared_img = tmp.path.join("shared").join("images");
        fs::create_dir_all(&week6_img).unwrap();
        fs::create_dir_all(&shared_img).unwrap();
        fs::write(week6_img.join("sky.png"), b"").unwrap();
        fs::write(shared_img.join("notes.png"), b"").unwrap();

        let mut locator = AssetLocator::new(&tmp.path);
        locator.set_current_level(Some("week6"));

        assert_eq!(locator.image("weeb/sky"), week6_img.join("sky.png"));
        assert_eq!(locator.image("notes"), shared_img.join("notes.png"));
        assert_eq!(
            locator.image("default:ui/popup/funkin/sick"),
            tmp.path.join("images/ui/popup/funkin/sick.png")
        );
    }

    #[test]
    fn resolves_song_metadata_and_chart_paths() {
        let locator = AssetLocator::new("assets");
        assert_eq!(
            locator.song_metadata("Bopeebo", None),
            PathBuf::from("assets/data/songs/bopeebo/bopeebo-metadata.json")
        );
        assert_eq!(
            locator.song_chart("Bopeebo", Some("erect")),
            PathBuf::from("assets/data/songs/bopeebo/bopeebo-chart-erect.json")
        );
        assert_eq!(
            locator.song_inst("Bopeebo", "-pico"),
            PathBuf::from("assets/songs/bopeebo/Inst-pico.ogg")
        );
    }

    #[test]
    fn resolves_erect_auto_variation_and_audio_stems() {
        let tmp = TempDir::new("song_variations");
        let data_dir = tmp.path.join("data").join("songs").join("spookeez");
        let audio_dir = tmp.path.join("songs").join("spookeez");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&audio_dir).unwrap();

        fs::write(
            data_dir.join("spookeez-metadata.json"),
            r#"{"playData":{"difficulties":["easy","normal","hard"]}}"#,
        )
        .unwrap();
        fs::write(
            data_dir.join("spookeez-metadata-erect.json"),
            r#"{"playData":{"difficulties":["erect","nightmare"],"characters":{"player":"bf-dark","opponent":"spooky-dark","instrumental":"erect","playerVocals":["bf-dark"],"opponentVocals":["spooky-dark"]}}}"#,
        )
        .unwrap();
        fs::write(
            data_dir.join("spookeez-metadata-pico.json"),
            r#"{"playData":{"difficulties":["easy","normal","hard"],"characters":{"player":"pico-dark","opponent":"spooky-dark","instrumental":"pico","playerVocals":["pico-dark"],"opponentVocals":["spooky-dark"]}}}"#,
        )
        .unwrap();

        fs::write(audio_dir.join("Inst.ogg"), b"").unwrap();
        fs::write(audio_dir.join("Inst-erect.ogg"), b"").unwrap();
        fs::write(audio_dir.join("Inst-pico.ogg"), b"").unwrap();
        fs::write(audio_dir.join("Voices-pico-pico.ogg"), b"").unwrap();
        fs::write(audio_dir.join("Voices-spooky-pico.ogg"), b"").unwrap();
        fs::write(audio_dir.join("Voices-bf-dark-erect.ogg"), b"").unwrap();

        let locator = AssetLocator::new(&tmp.path);

        assert_eq!(
            locator.resolve_song_variation("spookeez", "normal", None),
            None
        );
        assert_eq!(
            locator.resolve_song_variation("spookeez", "erect", None),
            Some("erect".to_owned())
        );
        assert_eq!(
            locator.resolve_song_variation("spookeez", "nightmare", None),
            Some("erect".to_owned())
        );
        assert_eq!(
            locator.resolve_song_variation("spookeez", "hard", Some("pico")),
            Some("pico".to_owned())
        );
        // `"bf"` falls back to base when `-metadata-bf.json` does not exist.
        assert_eq!(
            locator.resolve_song_variation("spookeez", "normal", Some("bf")),
            None
        );

        let pico_meta: SongMetadata = serde_json::from_str(
            &fs::read_to_string(data_dir.join("spookeez-metadata-pico.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            locator.resolve_song_inst("spookeez", Some(&pico_meta), Some("pico")),
            audio_dir.join("Inst-pico.ogg")
        );
        assert_eq!(
            locator.resolve_song_voices_for(
                "spookeez",
                pico_meta.play_data.characters.player_vocals.as_deref(),
                &pico_meta.play_data.characters.player,
                Some("pico"),
            ),
            Some(audio_dir.join("Voices-pico-pico.ogg"))
        );
        assert_eq!(
            locator.resolve_song_voices_for(
                "spookeez",
                pico_meta.play_data.characters.opponent_vocals.as_deref(),
                &pico_meta.play_data.characters.opponent,
                Some("pico"),
            ),
            Some(audio_dir.join("Voices-spooky-pico.ogg"))
        );
    }
}
