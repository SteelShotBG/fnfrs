use std::path::{Path, PathBuf};

use crate::constants::DEFAULT_VARIATION;

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
}
