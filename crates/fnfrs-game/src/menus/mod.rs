//! V-Slice menu states (`Title`, `MainMenu`, `StoryMenu`, `Freeplay`) with
//! zero per-frame heap allocations inside `update` / `draw`.

pub mod char_select;
pub mod credits;
pub mod freeplay;
pub mod main_menu;
pub mod options;
pub mod story_menu;
pub mod title;

use std::fs;
use std::path::Path;

use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::SongMetadata;
use fnfrs_render::{
    AtlasFont, AtlasFrame, BlendMode, FilterMode, GpuContext, SparrowAtlas, SpriteBatcher,
    SpriteInstance, TextureHandle, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y,
    SPRITE_FLAG_ROTATED_90_CCW,
};

pub use char_select::CharSelectScreenState;
pub use credits::CreditsScreenState;
pub use freeplay::FreeplayScreenState;
pub use main_menu::{MainMenuChoice, MainMenuScreenState};
pub use options::OptionsScreenState;
pub use story_menu::StoryMenuScreenState;
pub use title::TitleScreenState;

/// Common navigation/audio actions returned by menu state `update()` calls.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuCommand {
    None,
    PlaySfx(&'static str),
    PlaySfxVol(&'static str, f32),
    EnterMainMenu,
    EnterTitle,
    CancelToTitle,
    EnterStoryMenu,
    EnterFreeplay,
    EnterOptions,
    EnterCredits,
    ExitCreditsToMainMenu,
    EnterCharSelect,
    OpenUrl(&'static str),
    SavePreferencesChanged,
    StartFreeplaySong {
        song_id: String,
        difficulty: String,
        variation: Option<String>,
    },
    StartStoryWeek {
        level_id: String,
        level_title: String,
        songs: Vec<String>,
        difficulty: String,
    },
    PreviewSong {
        song_id: String,
        variation: Option<String>,
    },
    StopPreviewRestoreMenuMusic,
    RequestRestartMenuMusic,
}

/// Logical per-frame menu inputs sampled from `winit`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MenuInputState {
    pub up_pressed: bool,
    pub down_pressed: bool,
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub left_held: bool,
    pub right_held: bool,
    pub accept_pressed: bool,
    pub accept_held: bool,
    pub back_pressed: bool,
    pub tab_pressed: bool,
    pub fav_pressed: bool,
    pub char_select_pressed: bool,
    pub shift_held: bool,
}

/// Discovered playable Freeplay song metadata (populated once at menu init).
#[derive(Debug, Clone)]
pub struct FreeplaySongEntry {
    pub song_id: String,
    pub song_name: String,
    pub artist: String,
    pub bpm: f64,
    pub icon_id: String,
    pub base_difficulties: Vec<String>,
    pub erect_difficulties: Vec<String>,
    pub pico_difficulties: Vec<String>,
}

/// Uploads a PNG texture via [`AssetLocator::image`] if present on disk.
pub(crate) fn load_menu_image(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
) -> Option<(TextureHandle, [f32; 2])> {
    let path = assets.image(key);
    let tex = gpu.from_png_path_cached(Some(key), &path).ok()?;
    let (w, h) = gpu.texture_size(tex)?;
    Some((tex, [w as f32, h as f32]))
}

/// Loads a Sparrow atlas + texture via [`AssetLocator::image`] and [`AssetLocator::sparrow_xml`].
pub(crate) fn load_menu_sparrow(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
) -> Option<(TextureHandle, [f32; 2], SparrowAtlas)> {
    let (tex, sz) = load_menu_image(assets, gpu, key)?;
    let xml = fs::read_to_string(assets.sparrow_xml(key)).ok()?;
    let atlas = SparrowAtlas::from_xml(&xml).ok()?;
    Some((tex, sz, atlas))
}

/// Loads `images/fonts/bold` + `images/fonts/default` as a pre-indexed `AtlasFont`.
pub(crate) fn load_menu_atlas_font(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    prefer_bold: bool,
) -> AtlasFont {
    let bold_loaded = load_menu_sparrow(assets, gpu, "fonts/bold");
    let def_loaded = load_menu_sparrow(assets, gpu, "fonts/default");
    let bold_ref = bold_loaded
        .as_ref()
        .map(|(tex, sz, atlas)| (*tex, *sz, atlas));
    let def_ref = def_loaded
        .as_ref()
        .map(|(tex, sz, atlas)| (*tex, *sz, atlas));
    AtlasFont::from_sparrows(bold_ref, def_ref, prefer_bold)
}

/// Draws a single Sparrow frame in screen coordinates (top-left of untrimmed frame box).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_menu_sparrow_frame(
    batcher: &mut SpriteBatcher,
    tex: TextureHandle,
    tex_sz: [f32; 2],
    frame: &AtlasFrame,
    pos: [f32; 2],
    scale: [f32; 2],
    flip_x: bool,
    color: [f32; 4],
    hsv_shift: [f32; 3],
    filter: FilterMode,
) {
    let src = frame.source_size();
    let trimmed = frame.upright_trimmed_size();
    let trim_x = if flip_x {
        src[0] + frame.frame_x - trimmed[0]
    } else {
        -frame.frame_x
    };
    let trim_y = -frame.frame_y;
    let draw_pos = [pos[0] + trim_x * scale[0], pos[1] + trim_y * scale[1]];
    let draw_size = [trimmed[0] * scale[0], trimmed[1] * scale[1]];
    let mut flags = 0u32;
    if frame.rotated {
        flags |= SPRITE_FLAG_ROTATED_90_CCW;
    }
    if flip_x ^ frame.flip_x {
        flags |= SPRITE_FLAG_FLIP_X;
    }
    if frame.flip_y {
        flags |= SPRITE_FLAG_FLIP_Y;
    }
    let inset_px = if matches!(filter, FilterMode::Linear) {
        0.5
    } else {
        0.0
    };
    let inst = SpriteInstance::new(draw_pos, draw_size)
        .with_uv_rect(frame.uv_bounds_inset(tex_sz[0], tex_sz[1], inset_px))
        .with_color(color)
        .with_hsv_shift(hsv_shift)
        .with_flags(flags);
    batcher.push(tex, BlendMode::Normal, filter, inst);
}

/// Reads and parses all song metadata entries under `assets/data/songs/`.
#[must_use]
pub fn discover_freeplay_songs(assets: &AssetLocator) -> Vec<FreeplaySongEntry> {
    let songs_root = assets.root().join("data").join("songs");
    let mut entries = Vec::new();
    let Ok(read_dir) = fs::read_dir(&songs_root) else {
        return entries;
    };

    let mut dirs: Vec<(String, std::path::PathBuf)> = read_dir
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !p.is_dir() {
                return None;
            }
            let id = p.file_name()?.to_str()?.to_owned();
            Some((id, p))
        })
        .collect();
    dirs.sort_by_key(|(id, _)| song_sort_key(id));

    for (song_id, dir) in dirs {
        let base_meta = read_song_meta(&dir.join(format!("{song_id}-metadata.json")));
        let erect_meta = read_song_meta(&dir.join(format!("{song_id}-metadata-erect.json")));
        let pico_meta = read_song_meta(&dir.join(format!("{song_id}-metadata-pico.json")));

        let Some(primary) = base_meta.as_ref().or(erect_meta.as_ref()).or(pico_meta.as_ref())
        else {
            continue;
        };

        let base_difficulties = base_meta
            .as_ref()
            .map(|m| m.play_data.difficulties.clone())
            .unwrap_or_default();
        let erect_difficulties = erect_meta
            .as_ref()
            .map(|m| m.play_data.difficulties.clone())
            .unwrap_or_default();
        let pico_difficulties = pico_meta
            .as_ref()
            .map(|m| m.play_data.difficulties.clone())
            .unwrap_or_default();

        if base_difficulties.is_empty()
            && erect_difficulties.is_empty()
            && pico_difficulties.is_empty()
        {
            continue;
        }

        let bpm = primary
            .time_changes
            .first()
            .map_or(100.0, |t| t.bpm)
            .max(1.0);
        let icon_id = primary.play_data.characters.opponent.clone();

        entries.push(FreeplaySongEntry {
            song_id: song_id.clone(),
            song_name: if primary.song_name.is_empty() {
                song_id
            } else {
                primary.song_name.clone()
            },
            artist: primary.artist.clone(),
            bpm,
            icon_id,
            base_difficulties,
            erect_difficulties,
            pico_difficulties,
        });
    }

    entries
}

fn read_song_meta(path: &Path) -> Option<SongMetadata> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<SongMetadata>(&raw).ok()
}

fn song_sort_key(id: &str) -> (u8, String) {
    let lower = id.to_ascii_lowercase();
    let order = match lower.as_str() {
        "tutorial" => 0,
        "bopeebo" => 1,
        "fresh" => 2,
        "dadbattle" => 3,
        "spookeez" => 4,
        "south" => 5,
        "monster" => 6,
        "pico" => 7,
        "philly-nice" => 8,
        "blammed" => 9,
        "satin-panties" => 10,
        "high" => 11,
        "milf" => 12,
        "cocoa" => 13,
        "eggnog" => 14,
        "winter-horrorland" => 15,
        "senpai" => 16,
        "roses" => 17,
        "thorns" => 18,
        "ugh" => 19,
        "guns" => 20,
        "stress" => 21,
        "darnell" => 22,
        "lit-up" => 23,
        "2hot" => 24,
        "blazin" => 25,
        _ => 50,
    };
    (order, lower)
}

/// Canonical week display sort key matching V-Slice Story Mode order.
pub(crate) fn level_sort_key(id: &str) -> (u8, String) {
    let lower = id.to_ascii_lowercase();
    let rank = match lower.as_str() {
        "tutorial" => 0,
        "week1" => 1,
        "week2" => 2,
        "week3" => 3,
        "week4" => 4,
        "week5" => 5,
        "week6" => 6,
        "week7" => 7,
        "weekend1" => 8,
        "sserafim" => 9,
        _ => 20,
    };
    (rank, lower)
}
