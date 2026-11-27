//! Zero-allocation V-Slice Pause Substate (`PauseSubState.hx`).

use std::fmt::Write as _;

use fnfrs_core::constants::{DEFAULT_DIFFICULTY, VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AtlasFont, AtlasTextStyle, BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher,
    SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle,
};
use winit::keyboard::KeyCode;

use crate::camera::FlxEase;
use crate::menus::load_menu_atlas_font;

/// Active page / context inside [`PauseMenuState`] (`PauseMode` in `PauseSubState.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseMenuMode {
    #[default]
    Main,
    Difficulty,
    Conversation,
    Cutscene,
}

/// Pause menu options matching `PauseSubState.hx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseMenuOption {
    Resume,
    RestartSong,
    ChangeDifficulty,
    EnablePracticeMode,
    SkipCutscene,
    RestartCutscene,
    SkipDialogue,
    RestartDialogue,
    Exit,
}

impl PauseMenuOption {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Resume => "Resume",
            Self::RestartSong => "Restart Song",
            Self::ChangeDifficulty => "Change Difficulty",
            Self::EnablePracticeMode => "Enable Practice Mode",
            Self::SkipCutscene => "Skip Cutscene",
            Self::RestartCutscene => "Restart Cutscene",
            Self::SkipDialogue => "Skip Dialogue",
            Self::RestartDialogue => "Restart Dialogue",
            Self::Exit => "Exit to Menu",
        }
    }
}

/// Discrete per-frame input snapshot consumed by [`PauseMenuState::update`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuInput {
    pub up_pressed: bool,
    pub down_pressed: bool,
    pub up_held: bool,
    pub down_held: bool,
    pub shift_held: bool,
    pub accept_pressed: bool,
    pub back_pressed: bool,
}

impl PauseMenuInput {
    /// Maps a single key press event into a [`PauseMenuInput`].
    #[must_use]
    pub const fn from_key_code(code: KeyCode) -> Self {
        match code {
            KeyCode::ArrowUp | KeyCode::KeyW => Self {
                up_pressed: true,
                down_pressed: false,
                up_held: true,
                down_held: false,
                shift_held: false,
                accept_pressed: false,
                back_pressed: false,
            },
            KeyCode::ArrowDown | KeyCode::KeyS => Self {
                up_pressed: false,
                down_pressed: true,
                up_held: false,
                down_held: true,
                shift_held: false,
                accept_pressed: false,
                back_pressed: false,
            },
            KeyCode::Enter | KeyCode::Space => Self {
                up_pressed: false,
                down_pressed: false,
                up_held: false,
                down_held: false,
                shift_held: false,
                accept_pressed: true,
                back_pressed: false,
            },
            KeyCode::Escape | KeyCode::Backspace => Self {
                up_pressed: false,
                down_pressed: false,
                up_held: false,
                down_held: false,
                shift_held: false,
                accept_pressed: false,
                back_pressed: true,
            },
            _ => Self {
                up_pressed: false,
                down_pressed: false,
                up_held: false,
                down_held: false,
                shift_held: false,
                accept_pressed: false,
                back_pressed: false,
            },
        }
    }
}

/// High-level action emitted by [`PauseMenuState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseMenuAction {
    None,
    Resume,
    RestartSong,
    ChangeDifficulty(usize),
    TogglePractice,
    SkipCutscene,
    RestartCutscene,
    SkipDialogue,
    RestartDialogue,
    Exit,
}

const MAX_PAUSE_MENU_ITEMS: usize = 8;
const CHARTER_FADE_DELAY_S: f32 = 15.0;
const CHARTER_FADE_DUR_S: f32 = 0.75;
const MUSIC_FADE_IN_S: f32 = 5.0;
const MUSIC_FINAL_VOLUME: f32 = 0.75;

/// Zero-allocation V-Slice pause menu state (`PauseSubState.hx`).
#[derive(Debug, Clone)]
pub struct PauseMenuState {
    pub is_open: bool,
    pub mode: PauseMenuMode,
    pub selected: usize,
    pub open_time_s: f32,
    pub bg_alpha: f32,
    pub music_gain: f32,
    pub is_practice: bool,
    pub death_count: u32,
    pub global_offset_ms: f32,
    pub offset_dirty: bool,
    pub scroll_sfx_queued: bool,
    fast_offset: bool,
    offset_hold_s: f32,

    pub song_name: StackTextBuffer<64>,
    pub artist: StackTextBuffer<64>,
    pub charter: StackTextBuffer<64>,
    pub difficulty: StackTextBuffer<32>,
    difficulties: [StackTextBuffer<32>; MAX_PAUSE_MENU_ITEMS],
    difficulty_count: usize,
    item_x: [f32; MAX_PAUSE_MENU_ITEMS],
    item_y: [f32; MAX_PAUSE_MENU_ITEMS],
    bold_font: Option<AtlasFont>,
    just_opened: bool,
}

impl Default for PauseMenuState {
    fn default() -> Self {
        Self::new("Unknown Song", DEFAULT_DIFFICULTY)
    }
}

impl PauseMenuState {
    const MAIN_OPTIONS_FULL: [PauseMenuOption; 5] = [
        PauseMenuOption::Resume,
        PauseMenuOption::RestartSong,
        PauseMenuOption::ChangeDifficulty,
        PauseMenuOption::EnablePracticeMode,
        PauseMenuOption::Exit,
    ];
    const MAIN_OPTIONS_NO_PRACTICE: [PauseMenuOption; 4] = [
        PauseMenuOption::Resume,
        PauseMenuOption::RestartSong,
        PauseMenuOption::ChangeDifficulty,
        PauseMenuOption::Exit,
    ];
    const CUTSCENE_OPTIONS: [PauseMenuOption; 4] = [
        PauseMenuOption::Resume,
        PauseMenuOption::SkipCutscene,
        PauseMenuOption::RestartCutscene,
        PauseMenuOption::Exit,
    ];
    const CONVERSATION_OPTIONS: [PauseMenuOption; 4] = [
        PauseMenuOption::Resume,
        PauseMenuOption::SkipDialogue,
        PauseMenuOption::RestartDialogue,
        PauseMenuOption::Exit,
    ];

    /// Creates a new pause menu initialized for `song_name` and `difficulty`.
    #[must_use]
    pub fn new(song_name: &str, difficulty: &str) -> Self {
        let mut state = Self {
            is_open: false,
            mode: PauseMenuMode::Main,
            selected: 0,
            open_time_s: 0.0,
            bg_alpha: 0.0,
            music_gain: 0.0,
            is_practice: false,
            death_count: 0,
            global_offset_ms: 0.0,
            offset_dirty: false,
            scroll_sfx_queued: false,
            fast_offset: false,
            offset_hold_s: 0.0,
            song_name: StackTextBuffer::new(),
            artist: StackTextBuffer::new(),
            charter: StackTextBuffer::new(),
            difficulty: StackTextBuffer::new(),
            difficulties: std::array::from_fn(|_| StackTextBuffer::new()),
            difficulty_count: 0,
            item_x: [90.0; MAX_PAUSE_MENU_ITEMS],
            item_y: [VIRTUAL_HEIGHT * 0.48; MAX_PAUSE_MENU_ITEMS],
            bold_font: None,
            just_opened: false,
        };
        let _ = state.artist.write_str("Unknown");
        let _ = state.charter.write_str("Unknown");
        state.set_song_info(song_name, difficulty);
        state.set_difficulties(&["easy", "normal", "hard"]);
        state.snap_item_positions();
        state
    }

    /// Loads `fonts/bold` once at gameplay init so pause entries render as V-Slice bold AtlasText.
    pub fn load_atlas_font(&mut self, assets: &AssetLocator, gpu: &mut GpuContext) {
        let font = load_menu_atlas_font(assets, gpu, true);
        if font.is_loaded() {
            self.bold_font = Some(font);
        }
    }

    /// Updates displayed song metadata (`song_name`, `difficulty`).
    pub fn set_song_info(&mut self, song_name: &str, difficulty: &str) {
        self.song_name.clear();
        let _ = self.song_name.write_str(song_name);
        self.difficulty.clear();
        let _ = self.difficulty.write_str(difficulty);
    }

    /// Updates artist & charter metadata strings displayed cleanly top-right.
    pub fn set_credits(&mut self, artist: &str, charter: &str) {
        self.artist.clear();
        let a = if artist.trim().is_empty() { "Unknown" } else { artist.trim() };
        let _ = self.artist.write_str(a);
        self.charter.clear();
        let c = if charter.trim().is_empty() { "Unknown" } else { charter.trim() };
        let _ = self.charter.write_str(c);
    }

    /// Populates available difficulties for the `"Change Difficulty"` submenu.
    pub fn set_difficulties(&mut self, diffs: &[&str]) {
        let cap = (MAX_PAUSE_MENU_ITEMS - 1).min(diffs.len());
        self.difficulty_count = cap;
        for (i, d) in diffs.iter().take(cap).enumerate() {
            self.difficulties[i].clear();
            let _ = self.difficulties[i].write_str(d);
        }
    }

    /// Opens the standard song pause menu (`PauseSubState.create`).
    pub fn open(&mut self, _is_botplay: bool, _is_downscroll: bool, death_count: u32) {
        self.open_with_mode(PauseMenuMode::Main, self.is_practice, death_count, self.global_offset_ms);
    }

    /// Opens the pause menu in a specific `PauseMenuMode` (`Main`, `Conversation`, `Cutscene`).
    pub fn open_with_mode(
        &mut self,
        mode: PauseMenuMode,
        is_practice: bool,
        death_count: u32,
        global_offset_ms: f32,
    ) {
        self.is_open = true;
        self.mode = mode;
        self.selected = 0;
        self.open_time_s = 0.0;
        self.bg_alpha = 0.0;
        self.music_gain = 0.0;
        self.is_practice = is_practice;
        self.death_count = death_count;
        self.global_offset_ms = global_offset_ms;
        self.offset_dirty = false;
        self.scroll_sfx_queued = false;
        self.fast_offset = false;
        self.offset_hold_s = 0.0;
        self.just_opened = true;
        self.snap_item_positions();
    }

    /// Closes the pause menu.
    pub fn close(&mut self) {
        self.is_open = false;
        self.just_opened = false;
    }

    #[must_use]
    fn current_main_slice(&self) -> &[PauseMenuOption] {
        match self.mode {
            PauseMenuMode::Main => {
                if self.is_practice {
                    &Self::MAIN_OPTIONS_NO_PRACTICE
                } else {
                    &Self::MAIN_OPTIONS_FULL
                }
            }
            PauseMenuMode::Cutscene => &Self::CUTSCENE_OPTIONS,
            PauseMenuMode::Conversation => &Self::CONVERSATION_OPTIONS,
            PauseMenuMode::Difficulty => &[],
        }
    }

    /// Number of visible entries in the current [`PauseMenuMode`].
    #[must_use]
    pub fn item_count(&self) -> usize {
        match self.mode {
            PauseMenuMode::Difficulty => self.difficulty_count + 1,
            _ => self.current_main_slice().len(),
        }
    }

    /// Returns the option at `self.selected` when not in `Difficulty`.
    #[must_use]
    pub fn selected_main_option(&self) -> Option<PauseMenuOption> {
        self.current_main_slice().get(self.selected).copied()
    }

    /// Returns the difficulty string at index `idx` in [`PauseMenuMode::Difficulty`].
    #[must_use]
    pub fn difficulty_label(&self, idx: usize) -> Option<&str> {
        if idx < self.difficulty_count {
            Some(self.difficulties[idx].as_str())
        } else {
            None
        }
    }

    /// Moves selection cursor by `delta` with V-Slice wrap-around (`changeSelection`).
    pub fn move_selection(&mut self, delta: i32) {
        let len = self.item_count() as isize;
        if len <= 0 {
            self.selected = 0;
            return;
        }
        let prev = self.selected;
        let next = (self.selected as isize + delta as isize).rem_euclid(len);
        self.selected = next as usize;
        if self.selected != prev {
            self.scroll_sfx_queued = true;
        }
    }

    /// Advances animations and processes menu navigation with zero heap allocations.
    pub fn update(&mut self, elapsed_s: f32, input: PauseMenuInput) -> PauseMenuAction {
        if !self.is_open {
            return PauseMenuAction::None;
        }

        let dt = elapsed_s.clamp(0.0, 0.1);
        self.open_time_s += dt;
        self.music_gain = (self.open_time_s / MUSIC_FADE_IN_S).clamp(0.0, 1.0) * MUSIC_FINAL_VOLUME;
        let bg_t = (self.open_time_s / 0.8).clamp(0.0, 1.0);
        self.bg_alpha = FlxEase::QuartOut.eval(bg_t) * 0.6;

        // Smooth diagonal cascade matching `FlxMath.remapToRange((i - cur), 0, 1, 0, 1.3)`.
        let item_t = 1.0 - (-10.0 * dt).exp();
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let rel = (i as f32 - self.selected as f32) * 1.3;
            let target_x = rel * 20.0 + 90.0;
            let target_y = rel * 120.0 + VIRTUAL_HEIGHT * 0.48;
            self.item_x[i] += (target_x - self.item_x[i]) * item_t;
            self.item_y[i] += (target_y - self.item_y[i]) * item_t;
        }

        if self.just_opened {
            self.just_opened = false;
            return PauseMenuAction::None;
        }

        // Shift + Up/Down adjusts global audio offset (`PauseSubState.handleModifyingOffsets`).
        if input.shift_held && (input.up_held || input.down_held || input.up_pressed || input.down_pressed) {
            self.offset_hold_s += dt;
            let dir = if input.up_held || input.up_pressed { 1.0 } else { -1.0 };
            if !self.fast_offset {
                if self.offset_hold_s > 0.5 {
                    self.fast_offset = true;
                    self.offset_hold_s = 0.0;
                }
                if input.up_pressed || input.down_pressed {
                    self.global_offset_ms = (self.global_offset_ms + dir).clamp(-1500.0, 1500.0);
                    self.offset_dirty = true;
                }
            } else {
                self.global_offset_ms =
                    (self.global_offset_ms + dir * dt * 30.0).clamp(-1500.0, 1500.0);
                self.offset_dirty = true;
            }
            return PauseMenuAction::None;
        }
        self.fast_offset = false;
        self.offset_hold_s = 0.0;

        if input.up_pressed {
            self.move_selection(-1);
        }
        if input.down_pressed {
            self.move_selection(1);
        }

        if input.back_pressed {
            if self.mode == PauseMenuMode::Difficulty {
                self.mode = PauseMenuMode::Main;
                self.selected = 0;
                self.snap_item_positions();
                return PauseMenuAction::None;
            }
            self.close();
            return PauseMenuAction::Resume;
        }

        if input.accept_pressed {
            return self.activate_selected();
        }

        PauseMenuAction::None
    }

    fn activate_selected(&mut self) -> PauseMenuAction {
        if self.mode == PauseMenuMode::Difficulty {
            if self.selected >= self.difficulty_count {
                self.mode = PauseMenuMode::Main;
                self.selected = 0;
                self.snap_item_positions();
                return PauseMenuAction::None;
            }
            let chosen = self.selected;
            self.difficulty.clear();
            let _ = self.difficulty.write_str(self.difficulties[chosen].as_str());
            self.close();
            return PauseMenuAction::ChangeDifficulty(chosen);
        }

        let Some(opt) = self.selected_main_option() else {
            return PauseMenuAction::None;
        };
        match opt {
            PauseMenuOption::Resume => {
                self.close();
                PauseMenuAction::Resume
            }
            PauseMenuOption::RestartSong => {
                self.close();
                PauseMenuAction::RestartSong
            }
            PauseMenuOption::ChangeDifficulty => {
                self.mode = PauseMenuMode::Difficulty;
                self.selected = 0;
                self.snap_item_positions();
                PauseMenuAction::None
            }
            PauseMenuOption::EnablePracticeMode => {
                self.is_practice = true;
                if self.selected >= self.item_count() {
                    self.selected = 0;
                }
                self.snap_item_positions();
                PauseMenuAction::TogglePractice
            }
            PauseMenuOption::SkipCutscene => {
                self.close();
                PauseMenuAction::SkipCutscene
            }
            PauseMenuOption::RestartCutscene => {
                self.close();
                PauseMenuAction::RestartCutscene
            }
            PauseMenuOption::SkipDialogue => {
                self.close();
                PauseMenuAction::SkipDialogue
            }
            PauseMenuOption::RestartDialogue => {
                self.close();
                PauseMenuAction::RestartDialogue
            }
            PauseMenuOption::Exit => {
                self.close();
                PauseMenuAction::Exit
            }
        }
    }

    fn snap_item_positions(&mut self) {
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let rel = (i as f32 - self.selected as f32) * 1.3;
            self.item_x[i] = rel * 20.0 + 90.0;
            self.item_y[i] = rel * 120.0 + VIRTUAL_HEIGHT * 0.48;
        }
    }

    /// Computes staggered fade/drop for metadata lines (`transitionMetadataIn`: `delay = 0.1*(idx+1)`).
    fn metadata_anim(&self, idx: usize) -> (f32, f32) {
        let delay = 0.1 * (idx as f32 + 1.0);
        let t = ((self.open_time_s - delay) / 1.8).clamp(0.0, 1.0);
        let e = FlxEase::QuartOut.eval(t);
        (e, e * 5.0)
    }

    /// Renders the dimmed overlay, AtlasText options, and top/bottom-right metadata (zero allocs).
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if !self.is_open {
            return;
        }

        // 1. Semi-transparent black overlay.
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, self.bg_alpha]),
        );

        let is_small_font = font.line_height() <= 10.0;
        let fallback_scale = if is_small_font { 4.0 } else { 2.0 };
        let meta_scale = if is_small_font { 2.0 } else { 1.8 };
        let info_scale = if is_small_font { 1.0 } else { 0.95 };
        let mut line_buf = StackTextBuffer::<64>::new();

        // 2. Menu options (white bold `AtlasText`, selected `1.0` alpha, unselected `0.6`).
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let alpha = if i == self.selected { 1.0 } else { 0.6 };
            line_buf.clear();
            if self.mode == PauseMenuMode::Difficulty {
                if i < self.difficulty_count {
                    write_title_case(&mut line_buf, self.difficulties[i].as_str());
                } else {
                    let _ = line_buf.write_str("Back");
                }
            } else {
                let _ = line_buf.write_str(self.current_main_slice()[i].label());
            }

            let pos = [self.item_x[i].round(), self.item_y[i].round()];
            if let Some(atlas_font) = &self.bold_font {
                atlas_font.draw_bytes(
                    batcher,
                    line_buf.as_bytes(),
                    pos,
                    &AtlasTextStyle {
                        scale: 1.0,
                        color: [1.0, 1.0, 1.0, alpha],
                        align: TextAlign::Left,
                        time_s: self.open_time_s,
                        animated: false,
                        line_height: 70.0,
                    },
                );
            } else {
                font.draw_bytes_outlined(
                    batcher,
                    font_tex,
                    line_buf.as_bytes(),
                    pos,
                    &TextStyle {
                        scale: fallback_scale,
                        color: [1.0, 1.0, 1.0, alpha],
                        align: TextAlign::Left,
                        ..Default::default()
                    },
                    [0.0, 0.0, 0.0, alpha],
                );
            }
        }

        // 3. Top-right metadata (`PauseSubState.buildMetadata`).
        let meta_x = VIRTUAL_WIDTH - 20.0;
        let (a0, y0) = self.metadata_anim(0);
        if a0 > 0.01 {
            font.draw_bytes(
                batcher,
                font_tex,
                self.song_name.as_bytes(),
                meta_x,
                15.0 + y0,
                &TextStyle {
                    scale: meta_scale,
                    color: [1.0, 1.0, 1.0, a0],
                    align: TextAlign::Right,
                    ..Default::default()
                },
            );
        }

        // Artist / Charter swap every 15s (`CHARTER_FADE_DELAY`).
        let (a1, y1) = self.metadata_anim(1);
        let cycle = self.open_time_s % ((CHARTER_FADE_DELAY_S + CHARTER_FADE_DUR_S * 2.0) * 2.0);
        let half = CHARTER_FADE_DELAY_S + CHARTER_FADE_DUR_S * 2.0;
        let (show_charter, swap_alpha) = if cycle < CHARTER_FADE_DELAY_S {
            (false, 1.0)
        } else if cycle < CHARTER_FADE_DELAY_S + CHARTER_FADE_DUR_S {
            (false, 1.0 - FlxEase::QuartOut.eval((cycle - CHARTER_FADE_DELAY_S) / CHARTER_FADE_DUR_S))
        } else if cycle < half {
            (true, FlxEase::QuartOut.eval((cycle - CHARTER_FADE_DELAY_S - CHARTER_FADE_DUR_S) / CHARTER_FADE_DUR_S))
        } else if cycle < half + CHARTER_FADE_DELAY_S {
            (true, 1.0)
        } else if cycle < half + CHARTER_FADE_DELAY_S + CHARTER_FADE_DUR_S {
            (true, 1.0 - FlxEase::QuartOut.eval((cycle - half - CHARTER_FADE_DELAY_S) / CHARTER_FADE_DUR_S))
        } else {
            (false, FlxEase::QuartOut.eval((cycle - half - CHARTER_FADE_DELAY_S - CHARTER_FADE_DUR_S) / CHARTER_FADE_DUR_S))
        };
        let eff_a1 = a1 * swap_alpha.clamp(0.0, 1.0);
        if eff_a1 > 0.01 {
            line_buf.clear();
            if show_charter {
                let _ = write!(line_buf, "Charter: {}", self.charter.as_str());
            } else {
                let _ = write!(line_buf, "Artist: {}", self.artist.as_str());
            }
            font.draw_bytes(
                batcher,
                font_tex,
                line_buf.as_bytes(),
                meta_x,
                47.0 + y1,
                &TextStyle {
                    scale: meta_scale,
                    color: [1.0, 1.0, 1.0, eff_a1],
                    align: TextAlign::Right,
                    ..Default::default()
                },
            );
        }

        let (a2, y2) = self.metadata_anim(2);
        if a2 > 0.01 {
            line_buf.clear();
            let _ = line_buf.write_str("Difficulty: ");
            write_title_case(&mut line_buf, self.difficulty.as_str());
            font.draw_bytes(
                batcher,
                font_tex,
                line_buf.as_bytes(),
                meta_x,
                79.0 + y2,
                &TextStyle {
                    scale: meta_scale,
                    color: [1.0, 1.0, 1.0, a2],
                    align: TextAlign::Right,
                    ..Default::default()
                },
            );
        }

        let (a3, y3) = self.metadata_anim(3);
        if a3 > 0.01 {
            line_buf.clear();
            match self.mode {
                PauseMenuMode::Conversation => {
                    let _ = line_buf.write_str("Dialogue Paused");
                }
                PauseMenuMode::Cutscene => {
                    let _ = line_buf.write_str("Video Paused");
                }
                PauseMenuMode::Main | PauseMenuMode::Difficulty => {
                    let _ = write!(line_buf, "{} Blue Balls", self.death_count);
                }
            }
            font.draw_bytes(
                batcher,
                font_tex,
                line_buf.as_bytes(),
                meta_x,
                111.0 + y3,
                &TextStyle {
                    scale: meta_scale,
                    color: [1.0, 1.0, 1.0, a3],
                    align: TextAlign::Right,
                    ..Default::default()
                },
            );
        }

        if self.is_practice {
            let (a4, y4) = self.metadata_anim(4);
            if a4 > 0.01 {
                font.draw_bytes(
                    batcher,
                    font_tex,
                    b"PRACTICE MODE",
                    meta_x,
                    143.0 + y4,
                    &TextStyle {
                        scale: meta_scale,
                        color: [1.0, 1.0, 1.0, a4],
                        align: TextAlign::Right,
                        ..Default::default()
                    },
                );
            }
        }

        // 4. Bottom-right Global Offset indicator (`PauseSubState.hx:519-531`).
        let (a5, y5) = self.metadata_anim(5);
        if a5 > 0.01 {
            let off_y = VIRTUAL_HEIGHT - 76.0 + y5;
            line_buf.clear();
            let _ = write!(line_buf, "Global Offset: {}ms", self.global_offset_ms.round() as i32);
            font.draw_bytes(
                batcher,
                font_tex,
                line_buf.as_bytes(),
                meta_x,
                off_y,
                &TextStyle {
                    scale: info_scale,
                    color: [1.0, 1.0, 1.0, a5],
                    align: TextAlign::Right,
                    ..Default::default()
                },
            );
            font.draw_bytes(
                batcher,
                font_tex,
                b"Hold SHIFT-UP/DOWN,\nto change the offset.",
                meta_x,
                off_y + 20.0,
                &TextStyle {
                    scale: info_scale,
                    color: [1.0, 1.0, 1.0, a5],
                    align: TextAlign::Right,
                    ..Default::default()
                },
            );
        }
    }
}

fn write_title_case(buf: &mut StackTextBuffer<64>, raw: &str) {
    let mut cap_next = true;
    for ch in raw.chars() {
        if ch == '-' || ch == ' ' {
            let _ = buf.write_char(' ');
            cap_next = true;
        } else if cap_next {
            for u in ch.to_uppercase() {
                let _ = buf.write_char(u);
            }
            cap_next = false;
        } else {
            for l in ch.to_lowercase() {
                let _ = buf.write_char(l);
            }
        }
    }
}
