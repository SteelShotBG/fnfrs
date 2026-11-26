//! V-Slice Freeplay Song Browser (`FreeplayState.hx`) with zero per-frame allocations.

use std::fmt::Write as _;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AtlasFont, AtlasFrame, AtlasTextStyle, BitmapFont, BlendMode, FilterMode, GpuContext,
    SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle,
};

use super::{
    discover_freeplay_songs, draw_menu_sparrow_frame, load_menu_atlas_font, load_menu_image,
    load_menu_sparrow, FreeplaySongEntry, MenuCommand, MenuInputState,
};

const ALL_DIFF_LABELS: [&str; 5] = ["easy", "normal", "hard", "erect", "nightmare"];
const ALL_DIFF_UPPER: [&str; 5] = ["EASY", "NORMAL", "HARD", "ERECT", "NIGHTMARE"];

#[derive(Debug)]
struct CachedDiffOption {
    diff: &'static str,
    variation: Option<&'static str>,
}

const BASE_OPTS: [CachedDiffOption; 5] = [
    CachedDiffOption { diff: "easy", variation: None },
    CachedDiffOption { diff: "normal", variation: None },
    CachedDiffOption { diff: "hard", variation: None },
    CachedDiffOption { diff: "erect", variation: Some("erect") },
    CachedDiffOption { diff: "nightmare", variation: Some("erect") },
];

#[derive(Debug)]
pub struct FreeplayScreenState {
    white_tex: TextureHandle,
    bg_tex: Option<(TextureHandle, [f32; 2])>,
    pink_tex: Option<(TextureHandle, [f32; 2])>,
    capsule_tex: Option<(TextureHandle, [f32; 2])>,
    capsule_sel_frames: Vec<AtlasFrame>,
    capsule_idle_frames: Vec<AtlasFrame>,
    diff_textures: [Option<(TextureHandle, [f32; 2])>; 5],
    arrows_tex: Option<(TextureHandle, [f32; 2])>,
    left_arrow: Option<AtlasFrame>,
    right_arrow: Option<AtlasFrame>,

    songs: Vec<FreeplaySongEntry>,
    selected_song: usize,
    diff_idx: usize,
    pico_variation: bool,
    atlas_font: AtlasFont,
    playable_char: String,
    #[allow(dead_code)]
    fav_icon: Option<(TextureHandle, [f32; 2])>,
    pub char_select_requested: bool,
    pub favorite_toggle_requested: Option<(String, String)>,
    pub active_song_highscore: i64,
    pub active_song_rank: String,
    pub active_song_favorited: bool,

    scroll_y: f32,
    anim_time_s: f32,
    preview_pending: bool,
    preview_delay_s: f32,
    confirming: bool,
    confirm_timer_s: f32,
}

impl FreeplayScreenState {
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext) -> Self {
        let songs = discover_freeplay_songs(assets);
        let bg_tex = load_menu_image(assets, gpu, "freeplay/freeplayBGweek1-bf")
            .or_else(|| load_menu_image(assets, gpu, "menuDesat"));
        let pink_tex = load_menu_image(assets, gpu, "freeplay/pinkBack");

        let capsule_loaded =
            load_menu_sparrow(assets, gpu, "freeplay/freeplayCapsule/capsule/freeplayCapsule");
        let (capsule_tex, capsule_sel_frames, capsule_idle_frames) = match capsule_loaded {
            Some((t, sz, atlas)) => {
                let mut idle = Vec::new();
                let mut sel = Vec::new();
                for f in atlas.frames_by_prefix("mp3 capsule w backing") {
                    if f.name.contains("NOT SELECTED") {
                        idle.push(f.clone());
                    } else {
                        sel.push(f.clone());
                    }
                }
                (Some((t, sz)), sel, idle)
            }
            None => (None, Vec::new(), Vec::new()),
        };

        let diff_textures = [
            load_menu_image(assets, gpu, "freeplay/freeplayeasy"),
            load_menu_image(assets, gpu, "freeplay/freeplaynormal"),
            load_menu_image(assets, gpu, "freeplay/freeplayhard"),
            load_menu_image(assets, gpu, "freeplay/freeplayerect"),
            load_menu_image(assets, gpu, "freeplay/freeplaynightmare"),
        ];

        let arrows_loaded = load_menu_sparrow(assets, gpu, "storymenu/ui/arrows");
        let (arrows_tex, left_arrow, right_arrow) = match arrows_loaded {
            Some((t, sz, atlas)) => (
                Some((t, sz)),
                atlas.frames_by_prefix("leftIdle0").next().cloned(),
                atlas.frames_by_prefix("rightIdle0").next().cloned(),
            ),
            None => (None, None, None),
        };
        let atlas_font = load_menu_atlas_font(assets, gpu, true);
        let fav_icon = load_menu_image(assets, gpu, "freeplay/favHeart");

        Self {
            white_tex: gpu.white_texture(),
            bg_tex,
            pink_tex,
            capsule_tex,
            capsule_sel_frames,
            capsule_idle_frames,
            diff_textures,
            arrows_tex,
            left_arrow,
            right_arrow,
            songs,
            selected_song: 0,
            diff_idx: 1,
            pico_variation: false,
            atlas_font,
            playable_char: "bf".to_owned(),
            fav_icon,
            char_select_requested: false,
            favorite_toggle_requested: None,
            active_song_highscore: 0,
            active_song_rank: String::new(),
            active_song_favorited: false,
            scroll_y: 0.0,
            anim_time_s: 0.0,
            preview_pending: true,
            preview_delay_s: 0.25,
            confirming: false,
            confirm_timer_s: 0.0,
        }
    }

    /// Syncs selected playable character (`"bf"` / `"pico"`), saved highscore, and favorite state.
    pub fn sync_from_save(
        &mut self,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        save: &fnfrs_core::save::SaveData,
    ) {
        if !self.playable_char.eq_ignore_ascii_case(&save.playable_character) {
            self.playable_char = save.playable_character.clone();
            self.pico_variation = self.playable_char.eq_ignore_ascii_case("pico");
            let bg_key = if self.pico_variation {
                "freeplay/freeplayBGweek1-pico"
            } else {
                "freeplay/freeplayBGweek1-bf"
            };
            if let Some(new_bg) = load_menu_image(assets, gpu, bg_key) {
                self.bg_tex = Some(new_bg);
            }
            self.snap_difficulty_to_available(0);
        }
        if let Some(song) = self.songs.get(self.selected_song) {
            let var = self.effective_variation(song);
            let diff = ALL_DIFF_LABELS[self.diff_idx];
            if let Some(rec) = save.get_song_score(&song.song_id, var.as_deref(), diff) {
                self.active_song_highscore = rec.score;
                self.active_song_rank = rec.rank.clone();
            } else {
                self.active_song_highscore = 0;
                self.active_song_rank.clear();
            }
            self.active_song_favorited = save.is_favorite(&song.song_id);
        }
    }

    pub fn reset_for_enter(&mut self) {
        self.confirming = false;
        self.confirm_timer_s = 0.0;
        self.scroll_y = self.selected_song as f32 * 116.0;
        self.preview_pending = true;
        self.preview_delay_s = 0.25;
        self.snap_difficulty_to_available(0);
    }

    fn current_song_supports(&self, song: &FreeplaySongEntry, opt: &CachedDiffOption) -> bool {
        if self.pico_variation {
            song.pico_difficulties
                .iter()
                .any(|d| d.eq_ignore_ascii_case(opt.diff))
        } else if opt.variation == Some("erect") {
            song.erect_difficulties
                .iter()
                .any(|d| d.eq_ignore_ascii_case(opt.diff))
        } else {
            song.base_difficulties
                .iter()
                .any(|d| d.eq_ignore_ascii_case(opt.diff))
        }
    }

    fn snap_difficulty_to_available(&mut self, dir: isize) {
        let Some(song) = self.songs.get(self.selected_song) else {
            return;
        };
        if dir == 0 && self.current_song_supports(song, &BASE_OPTS[self.diff_idx]) {
            return;
        }
        let step: isize = if dir < 0 { -1 } else { 1 };
        let len = BASE_OPTS.len() as isize;
        let mut idx = self.diff_idx as isize;
        for _ in 0..len {
            if dir != 0 {
                idx = (idx + step).rem_euclid(len);
            }
            if self.current_song_supports(song, &BASE_OPTS[idx as usize]) {
                self.diff_idx = idx as usize;
                return;
            }
            if dir == 0 {
                idx = (idx + 1).rem_euclid(len);
            }
        }
        if self.pico_variation && song.pico_difficulties.is_empty() {
            self.pico_variation = false;
            self.snap_difficulty_to_available(0);
        }
    }

    fn effective_variation(&self, song: &FreeplaySongEntry) -> Option<String> {
        if self.pico_variation
            && song
                .pico_difficulties
                .iter()
                .any(|d| d.eq_ignore_ascii_case(ALL_DIFF_LABELS[self.diff_idx]))
        {
            Some("pico".to_owned())
        } else {
            BASE_OPTS[self.diff_idx].variation.map(str::to_owned)
        }
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        let dt = elapsed_s.clamp(0.0, 0.1);
        self.anim_time_s += dt;

        let target_scroll = self.selected_song as f32 * 116.0;
        let lerp_t = 1.0 - (1.0 - 0.18_f32).powf(dt * 60.0);
        self.scroll_y += (target_scroll - self.scroll_y) * lerp_t;

        if self.confirming {
            self.confirm_timer_s += dt;
            if self.confirm_timer_s >= 0.35 {
                self.confirming = false;
                if let Some(song) = self.songs.get(self.selected_song) {
                    return MenuCommand::StartFreeplaySong {
                        song_id: song.song_id.clone(),
                        difficulty: ALL_DIFF_LABELS[self.diff_idx].to_owned(),
                        variation: self.effective_variation(song),
                    };
                }
            }
            return MenuCommand::None;
        }

        if input.back_pressed {
            return MenuCommand::StopPreviewRestoreMenuMusic;
        }

        if input.up_pressed && !self.songs.is_empty() {
            self.selected_song = (self.selected_song + self.songs.len() - 1) % self.songs.len();
            self.snap_difficulty_to_available(0);
            self.preview_pending = true;
            self.preview_delay_s = 0.25;
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.down_pressed && !self.songs.is_empty() {
            self.selected_song = (self.selected_song + 1) % self.songs.len();
            self.snap_difficulty_to_available(0);
            self.preview_pending = true;
            self.preview_delay_s = 0.25;
            return MenuCommand::PlaySfx("scrollMenu");
        }

        if input.left_pressed && !self.songs.is_empty() {
            let prev_var = self.effective_variation(&self.songs[self.selected_song]);
            self.snap_difficulty_to_available(-1);
            let next_var = self.effective_variation(&self.songs[self.selected_song]);
            if prev_var != next_var {
                self.preview_pending = true;
                self.preview_delay_s = 0.15;
            }
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.right_pressed && !self.songs.is_empty() {
            let prev_var = self.effective_variation(&self.songs[self.selected_song]);
            self.snap_difficulty_to_available(1);
            let next_var = self.effective_variation(&self.songs[self.selected_song]);
            if prev_var != next_var {
                self.preview_pending = true;
                self.preview_delay_s = 0.15;
            }
            return MenuCommand::PlaySfx("scrollMenu");
        }

        if input.tab_pressed && !self.songs.is_empty() {
            let song = &self.songs[self.selected_song];
            if !song.pico_difficulties.is_empty() {
                self.pico_variation = !self.pico_variation;
                self.snap_difficulty_to_available(0);
                self.preview_pending = true;
                self.preview_delay_s = 0.15;
                return MenuCommand::PlaySfx("scrollMenu");
            }
        }

        if input.fav_pressed && !self.songs.is_empty() {
            let song = &self.songs[self.selected_song];
            let variation = self
                .effective_variation(song)
                .unwrap_or_else(|| "default".to_owned());
            self.favorite_toggle_requested = Some((song.song_id.clone(), variation));
            return MenuCommand::PlaySfx("scrollMenu");
        }

        if input.char_select_pressed {
            self.char_select_requested = true;
            return MenuCommand::PlaySfx("confirmMenu");
        }

        if input.accept_pressed && !self.songs.is_empty() {
            self.confirming = true;
            self.confirm_timer_s = 0.0;
            return MenuCommand::PlaySfx("confirmMenu");
        }

        if self.preview_pending {
            self.preview_delay_s -= dt;
            if self.preview_delay_s <= 0.0 {
                self.preview_pending = false;
                if let Some(song) = self.songs.get(self.selected_song) {
                    return MenuCommand::PreviewSong {
                        song_id: song.song_id.clone(),
                        variation: self.effective_variation(song),
                    };
                }
            }
        }

        MenuCommand::None
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        if let Some((tex, sz)) = self.bg_tex {
            let scale = (VIRTUAL_WIDTH / sz[0].max(1.0)).max(VIRTUAL_HEIGHT / sz[1].max(1.0));
            let w = sz[0] * scale;
            let h = sz[1] * scale;
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([(VIRTUAL_WIDTH - w) * 0.5, (VIRTUAL_HEIGHT - h) * 0.5], [w, h])
                    .with_color([0.92, 0.72, 0.96, 1.0]),
            );
        } else {
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([0.16, 0.11, 0.25, 1.0]),
            );
        }

        if let Some((tex, sz)) = self.pink_tex {
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([0.0, 0.0], [sz[0], VIRTUAL_HEIGHT.max(sz[1])]),
            );
        }

        // Top dark header strip.
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 64.0])
                .with_color([0.0, 0.0, 0.0, 0.85]),
        );

        if self.atlas_font.is_loaded() {
            self.atlas_font.draw_str(
                batcher,
                "FREEPLAY",
                [28.0, 12.0],
                &AtlasTextStyle {
                    scale: 0.62,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Left,
                    time_s: self.anim_time_s,
                    animated: true,
                    ..Default::default()
                },
            );
        } else {
            font.draw_str(
                batcher,
                font_tex,
                "FREEPLAY",
                28.0,
                16.0,
                &TextStyle {
                    scale: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Left,
                    ..Default::default()
                },
            );
        }

        if self.songs.is_empty() {
            font.draw_str(
                batcher,
                font_tex,
                "NO SONGS FOUND IN assets/data/songs/",
                VIRTUAL_WIDTH * 0.5,
                VIRTUAL_HEIGHT * 0.5,
                &TextStyle {
                    scale: 1.1,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
            return;
        }

        // Render visible song capsules along the right half (`x = 460`).
        for (idx, song) in self.songs.iter().enumerate() {
            let y = 260.0 + (idx as f32) * 116.0 - self.scroll_y;
            if y < 60.0 || y > VIRTUAL_HEIGHT + 20.0 {
                continue;
            }
            let is_sel = idx == self.selected_song;
            let dist = ((idx as f32) - (self.selected_song as f32)).abs();
            let x = 490.0 - (dist * 12.0).min(70.0);
            let alpha = if is_sel { 1.0 } else { 0.65 };

            if let Some((tex, sz)) = self.capsule_tex {
                let frames = if is_sel && !self.capsule_sel_frames.is_empty() {
                    &self.capsule_sel_frames
                } else {
                    &self.capsule_idle_frames
                };
                if let Some(frame) = frames
                    .get(((self.anim_time_s * 24.0).floor() as usize) % frames.len().max(1))
                {
                    draw_menu_sparrow_frame(
                        batcher,
                        tex,
                        sz,
                        frame,
                        [x, y],
                        [0.95, 0.95],
                        false,
                        [1.0, 1.0, 1.0, alpha],
                        [0.0, 0.0, 0.0],
                        FilterMode::Linear,
                    );
                }
            } else {
                batcher.push(
                    self.white_tex,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([x, y], [560.0, 92.0]).with_color(if is_sel {
                        [0.18, 0.14, 0.34, 0.95]
                    } else {
                        [0.08, 0.06, 0.16, 0.7]
                    }),
                );
            }

            if self.atlas_font.is_loaded() {
                self.atlas_font.draw_str(
                    batcher,
                    &song.song_name,
                    [x + 86.0, y + 26.0],
                    &AtlasTextStyle {
                        scale: if is_sel { 0.56 } else { 0.48 },
                        color: [1.0, 1.0, 1.0, alpha],
                        align: TextAlign::Left,
                        time_s: self.anim_time_s,
                        animated: is_sel,
                        ..Default::default()
                    },
                );
            } else {
                font.draw_str(
                    batcher,
                    font_tex,
                    &song.song_name,
                    x + 92.0,
                    y + 38.0,
                    &TextStyle {
                        scale: if is_sel { 1.0 } else { 0.88 },
                        color: [1.0, 1.0, 1.0, alpha],
                        align: TextAlign::Left,
                        ..Default::default()
                    },
                );
            }
        }

        // Left side difficulty badge & info card (`x = 70..390`).
        let sel_song = &self.songs[self.selected_song];
        if let Some((tex, sz)) = self.arrows_tex {
            if let Some(lf) = &self.left_arrow {
                draw_menu_sparrow_frame(
                    batcher,
                    tex,
                    sz,
                    lf,
                    [70.0, 140.0],
                    [0.7, 0.7],
                    false,
                    [1.0, 1.0, 1.0, 1.0],
                    [0.0, 0.0, 0.0],
                    FilterMode::Linear,
                );
            }
            if let Some(rf) = &self.right_arrow {
                draw_menu_sparrow_frame(
                    batcher,
                    tex,
                    sz,
                    rf,
                    [360.0, 140.0],
                    [0.7, 0.7],
                    false,
                    [1.0, 1.0, 1.0, 1.0],
                    [0.0, 0.0, 0.0],
                    FilterMode::Linear,
                );
            }
        }
        if let Some((dtex, dsz)) = self.diff_textures[self.diff_idx] {
            let scale = (220.0 / dsz[0].max(1.0)).min(1.0);
            let w = dsz[0] * scale;
            let h = dsz[1] * scale;
            batcher.push(
                dtex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([232.0 - w * 0.5, 170.0 - h * 0.5], [w, h]),
            );
        } else {
            font.draw_str(
                batcher,
                font_tex,
                ALL_DIFF_UPPER[self.diff_idx],
                232.0,
                156.0,
                &TextStyle {
                    scale: 1.05,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }

        // Metadata info box on left side.
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([40.0, 260.0], [380.0, 240.0])
                .with_color([0.05, 0.04, 0.10, 0.72]),
        );

        let mut line_buf = StackTextBuffer::<96>::new();
        let _ = write!(line_buf, "Artist: {}", sel_song.artist);
        font.draw_bytes(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            60.0,
            286.0,
            &TextStyle {
                scale: 0.72,
                color: [0.95, 0.95, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
        );
        line_buf.clear();
        let _ = write!(line_buf, "BPM: {:.0}", sel_song.bpm);
        font.draw_bytes(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            60.0,
            326.0,
            &TextStyle {
                scale: 0.72,
                color: [0.95, 0.95, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
        );
        line_buf.clear();
        let var_name = if self.effective_variation(sel_song).as_deref() == Some("pico") {
            "Mix: Pico (Tab)"
        } else if !sel_song.pico_difficulties.is_empty() {
            "Mix: Default (Tab for Pico)"
        } else {
            "Mix: Standard"
        };
        font.draw_str(
            batcher,
            font_tex,
            var_name,
            60.0,
            366.0,
            &TextStyle {
                scale: 0.72,
                color: [0.75, 0.95, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
        );
        line_buf.clear();
        let _ = write!(line_buf, "High Score: {}", self.active_song_highscore);
        font.draw_bytes(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            60.0,
            406.0,
            &TextStyle {
                scale: 0.72,
                color: [1.0, 0.92, 0.35, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
        );
        let fav_str = if self.active_song_favorited {
            "[F] Favorited (*)"
        } else {
            "[F] Favorite  |  [C] Char Select"
        };
        font.draw_str(
            batcher,
            font_tex,
            fav_str,
            60.0,
            446.0,
            &TextStyle {
                scale: 0.65,
                color: if self.active_song_favorited {
                    [1.0, 0.45, 0.65, 1.0]
                } else {
                    [0.85, 0.85, 0.92, 0.9]
                },
                align: TextAlign::Left,
                ..Default::default()
            },
        );

        line_buf.clear();
        let _ = write!(line_buf, "Song {} / {}", self.selected_song + 1, self.songs.len());
        font.draw_bytes(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            VIRTUAL_WIDTH - 24.0,
            20.0,
            &TextStyle {
                scale: 0.8,
                color: [0.85, 0.9, 1.0, 0.9],
                align: TextAlign::Right,
                ..Default::default()
            },
        );
    }
}
