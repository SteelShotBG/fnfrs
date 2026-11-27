use std::fmt::Write as _;

use fnfrs_core::constants::{DEFAULT_DIFFICULTY, VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign,
    TextStyle, TextureHandle,
};
use winit::keyboard::KeyCode;
/// Active page inside [`PauseMenuState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseMenuMode {
    #[default]
    Main,
    Difficulty,
}

/// Standard V-Slice pause menu options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseMenuOption {
    Resume,
    RestartSong,
    ChangeDifficulty,
    TogglePractice,
    ToggleBotplay,
    ToggleDownscroll,
    Exit,
}

impl PauseMenuOption {
    /// Human-readable label shown on screen.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Resume => "Resume",
            Self::RestartSong => "Restart Song",
            Self::ChangeDifficulty => "Change Difficulty",
            Self::TogglePractice => "Toggle Practice Mode",
            Self::ToggleBotplay => "Toggle Botplay",
            Self::ToggleDownscroll => "Toggle Downscroll",
            Self::Exit => "Exit",
        }
    }
}

/// Discrete input snapshot consumed by [`PauseMenuState::update`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuInput {
    pub up_pressed: bool,
    pub down_pressed: bool,
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
                accept_pressed: false,
                back_pressed: false,
            },
            KeyCode::ArrowDown | KeyCode::KeyS => Self {
                up_pressed: false,
                down_pressed: true,
                accept_pressed: false,
                back_pressed: false,
            },
            KeyCode::Enter | KeyCode::Space => Self {
                up_pressed: false,
                down_pressed: false,
                accept_pressed: true,
                back_pressed: false,
            },
            KeyCode::Escape | KeyCode::Backspace => Self {
                up_pressed: false,
                down_pressed: false,
                accept_pressed: false,
                back_pressed: true,
            },
            _ => Self {
                up_pressed: false,
                down_pressed: false,
                accept_pressed: false,
                back_pressed: false,
            },
        }
    }
}

/// High-level action emitted by [`PauseMenuState::update`] when the player confirms an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseMenuAction {
    None,
    Resume,
    RestartSong,
    ChangeDifficulty(usize),
    TogglePractice,
    ToggleBotplay,
    ToggleDownscroll,
    Exit,
}

const MAX_PAUSE_MENU_ITEMS: usize = 8;

/// Zero-allocation V-Slice pause menu state (`PauseSubState.hx`).
#[derive(Debug, Clone)]
pub struct PauseMenuState {
    pub is_open: bool,
    pub mode: PauseMenuMode,
    pub selected: usize,
    pub bg_alpha: f32,
    pub metadata_alpha: f32,
    pub is_practice: bool,
    pub is_botplay: bool,
    pub is_downscroll: bool,
    pub death_count: u32,
    pub song_name: StackTextBuffer<64>,
    pub difficulty: StackTextBuffer<32>,
    difficulties: [StackTextBuffer<32>; MAX_PAUSE_MENU_ITEMS],
    difficulty_count: usize,
    item_x: [f32; MAX_PAUSE_MENU_ITEMS],
    item_y: [f32; MAX_PAUSE_MENU_ITEMS],
    just_opened: bool,
}

impl Default for PauseMenuState {
    fn default() -> Self {
        Self::new("Unknown Song", DEFAULT_DIFFICULTY)
    }
}

impl PauseMenuState {
    const MAIN_OPTIONS: [PauseMenuOption; 7] = [
        PauseMenuOption::Resume,
        PauseMenuOption::RestartSong,
        PauseMenuOption::ChangeDifficulty,
        PauseMenuOption::TogglePractice,
        PauseMenuOption::ToggleBotplay,
        PauseMenuOption::ToggleDownscroll,
        PauseMenuOption::Exit,
    ];

    /// Creates a new pause menu initialized for `song_name` and `difficulty`.
    #[must_use]
    pub fn new(song_name: &str, difficulty: &str) -> Self {
        let mut state = Self {
            is_open: false,
            mode: PauseMenuMode::Main,
            selected: 0,
            bg_alpha: 0.0,
            metadata_alpha: 0.0,
            is_practice: false,
            is_botplay: false,
            is_downscroll: false,
            death_count: 0,
            song_name: StackTextBuffer::new(),
            difficulty: StackTextBuffer::new(),
            difficulties: std::array::from_fn(|_| StackTextBuffer::new()),
            difficulty_count: 0,
            item_x: [90.0; MAX_PAUSE_MENU_ITEMS],
            item_y: [VIRTUAL_HEIGHT * 0.48; MAX_PAUSE_MENU_ITEMS],
            just_opened: false,
        };
        state.set_song_info(song_name, difficulty);
        state.set_difficulties(&["easy", "normal", "hard"]);
        state.snap_item_positions();
        state
    }

    /// Updates displayed song metadata with zero heap allocations.
    pub fn set_song_info(&mut self, song_name: &str, difficulty: &str) {
        self.song_name.clear();
        let _ = self.song_name.write_str(song_name);
        self.difficulty.clear();
        let _ = self.difficulty.write_str(difficulty);
    }

    /// Populates available difficulties for the `"Change Difficulty"` submenu.
    pub fn set_difficulties(&mut self, diffs: &[&str]) {
        // Reserve 1 slot for trailing "Back".
        let cap = (MAX_PAUSE_MENU_ITEMS - 1).min(diffs.len());
        self.difficulty_count = cap;
        for (i, d) in diffs.iter().take(cap).enumerate() {
            self.difficulties[i].clear();
            let _ = self.difficulties[i].write_str(d);
        }
    }

    /// Opens the pause menu and starts the fade-in transition (`PauseSubState.create`).
    pub fn open(&mut self, is_botplay: bool, is_downscroll: bool, death_count: u32) {
        self.is_open = true;
        self.mode = PauseMenuMode::Main;
        self.selected = 0;
        self.bg_alpha = 0.0;
        self.metadata_alpha = 0.0;
        self.is_botplay = is_botplay;
        self.is_downscroll = is_downscroll;
        self.death_count = death_count;
        self.just_opened = true;
        self.snap_item_positions();
    }

    /// Closes the pause menu.
    pub fn close(&mut self) {
        self.is_open = false;
        self.just_opened = false;
    }

    /// Number of visible entries in the current [`PauseMenuMode`].
    #[must_use]
    pub fn item_count(&self) -> usize {
        match self.mode {
            PauseMenuMode::Main => Self::MAIN_OPTIONS.len(),
            PauseMenuMode::Difficulty => self.difficulty_count + 1,
        }
    }

    /// Returns the main-menu option at `self.selected` when in [`PauseMenuMode::Main`].
    #[must_use]
    pub fn selected_main_option(&self) -> Option<PauseMenuOption> {
        if self.mode == PauseMenuMode::Main {
            Self::MAIN_OPTIONS.get(self.selected).copied()
        } else {
            None
        }
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
        let next = (self.selected as isize + delta as isize).rem_euclid(len);
        self.selected = next as usize;
    }

    /// Advances animations and processes menu navigation with zero heap allocations.
    pub fn update(&mut self, elapsed_s: f32, input: PauseMenuInput) -> PauseMenuAction {
        if !self.is_open {
            return PauseMenuAction::None;
        }

        let dt = elapsed_s.max(0.0);
        // Background dims smoothly toward 0.6 (`PauseSubState.transitionIn`).
        let alpha_t = 1.0 - (-4.5 * dt).exp();
        self.bg_alpha += (0.6 - self.bg_alpha) * alpha_t;
        self.metadata_alpha += (1.0 - self.metadata_alpha) * alpha_t;

        // Smooth diagonal menu cascade (`PauseSubState.changeSelection`).
        let item_t = 1.0 - (-12.0 * dt).exp();
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let rel = i as f32 - self.selected as f32;
            let target_x = rel * 26.0 + 90.0;
            let target_y = rel * 110.0 + VIRTUAL_HEIGHT * 0.48;
            self.item_x[i] += (target_x - self.item_x[i]) * item_t;
            self.item_y[i] += (target_y - self.item_y[i]) * item_t;
        }

        if input.up_pressed {
            self.move_selection(-1);
        }
        if input.down_pressed {
            self.move_selection(1);
        }

        // Guard against closing on the exact frame the pause key opened the menu.
        if self.just_opened {
            self.just_opened = false;
            return PauseMenuAction::None;
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
        match self.mode {
            PauseMenuMode::Main => {
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
                    PauseMenuOption::TogglePractice => {
                        self.is_practice = !self.is_practice;
                        PauseMenuAction::TogglePractice
                    }
                    PauseMenuOption::ToggleBotplay => {
                        self.is_botplay = !self.is_botplay;
                        PauseMenuAction::ToggleBotplay
                    }
                    PauseMenuOption::ToggleDownscroll => {
                        self.is_downscroll = !self.is_downscroll;
                        PauseMenuAction::ToggleDownscroll
                    }
                    PauseMenuOption::Exit => {
                        self.close();
                        PauseMenuAction::Exit
                    }
                }
            }
            PauseMenuMode::Difficulty => {
                if self.selected >= self.difficulty_count {
                    self.mode = PauseMenuMode::Main;
                    self.selected = 0;
                    self.snap_item_positions();
                    PauseMenuAction::None
                } else {
                    let chosen = self.selected;
                    if chosen < self.difficulty_count {
                        self.difficulty.clear();
                        let _ = self.difficulty.write_str(self.difficulties[chosen].as_str());
                    }
                    self.close();
                    PauseMenuAction::ChangeDifficulty(chosen)
                }
            }
        }
    }

    fn snap_item_positions(&mut self) {
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let rel = i as f32 - self.selected as f32;
            self.item_x[i] = rel * 26.0 + 90.0;
            self.item_y[i] = rel * 110.0 + VIRTUAL_HEIGHT * 0.48;
        }
    }

    /// Renders the dimmed overlay, scrolling option entries, and top-right metadata with
    /// zero heap allocations.
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
        let entry_scale = if is_small_font { 4.0 } else { 2.0 };
        let meta_scale = if is_small_font { 2.0 } else { 1.1 };
        let mut line_buf = StackTextBuffer::<64>::new();

        // 2. Menu options.
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let is_cur = i == self.selected;
            let alpha = if is_cur { 1.0 } else { 0.55 };
            let color = if is_cur {
                [1.0, 0.95, 0.45, alpha]
            } else {
                [1.0, 1.0, 1.0, alpha]
            };

            line_buf.clear();
            match self.mode {
                PauseMenuMode::Main => {
                    let opt = Self::MAIN_OPTIONS[i];
                    match opt {
                        PauseMenuOption::TogglePractice => {
                            let state_str = if self.is_practice { "ON" } else { "OFF" };
                            let _ = write!(line_buf, "Practice Mode: {state_str}");
                        }
                        PauseMenuOption::ToggleBotplay => {
                            let state_str = if self.is_botplay { "ON" } else { "OFF" };
                            let _ = write!(line_buf, "Botplay: {state_str}");
                        }
                        PauseMenuOption::ToggleDownscroll => {
                            let state_str = if self.is_downscroll { "ON" } else { "OFF" };
                            let _ = write!(line_buf, "Downscroll: {state_str}");
                        }
                        _ => {
                            let _ = line_buf.write_str(opt.label());
                        }
                    }
                }
                PauseMenuMode::Difficulty => {
                    if i < self.difficulty_count {
                        let _ = line_buf.write_str(self.difficulties[i].as_str());
                    } else {
                        let _ = line_buf.write_str("Back");
                    }
                }
            }

            let style = TextStyle {
                scale: entry_scale,
                color,
                align: TextAlign::Left,
                ..Default::default()
            };
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                line_buf.as_bytes(),
                [self.item_x[i].round(), self.item_y[i].round()],
                &style,
                [0.0, 0.0, 0.0, alpha],
            );
        }

        // 3. Top-right metadata (`PauseSubState.buildMetadata`).
        let meta_x = VIRTUAL_WIDTH - 32.0;
        let meta_style = TextStyle {
            scale: meta_scale,
            color: [1.0, 1.0, 1.0, self.metadata_alpha],
            align: TextAlign::Right,
            ..Default::default()
        };

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            self.song_name.as_bytes(),
            [meta_x, 22.0],
            &meta_style,
            [0.0, 0.0, 0.0, self.metadata_alpha],
        );

        line_buf.clear();
        let _ = write!(line_buf, "Difficulty: {}", self.difficulty.as_str());
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            [meta_x, 56.0],
            &meta_style,
            [0.0, 0.0, 0.0, self.metadata_alpha],
        );

        line_buf.clear();
        let _ = write!(line_buf, "{} Blue Balls", self.death_count);
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            [meta_x, 90.0],
            &meta_style,
            [0.0, 0.0, 0.0, self.metadata_alpha],
        );
    }
}
