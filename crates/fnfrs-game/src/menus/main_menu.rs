//! V-Slice Main Menu (`MainMenuState.hx`).

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AtlasFrame, BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance,
    TextAlign, TextStyle, TextureHandle,
};

use super::{draw_menu_sparrow_frame, load_menu_image, load_menu_sparrow, MenuCommand, MenuInputState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainMenuChoice {
    StoryMode,
    Freeplay,
    Options,
    Credits,
}

#[derive(Debug)]
struct MenuItemEntry {
    choice: MainMenuChoice,
    label: &'static str,
    tex: Option<(TextureHandle, [f32; 2])>,
    idle_frames: Vec<AtlasFrame>,
    selected_frames: Vec<AtlasFrame>,
    base_y: f32,
}

#[derive(Debug)]
pub struct MainMenuScreenState {
    white_tex: TextureHandle,
    bg_tex: Option<(TextureHandle, [f32; 2])>,
    magenta_tex: Option<(TextureHandle, [f32; 2])>,
    items: Vec<MenuItemEntry>,
    selected: usize,
    anim_time_s: f32,
    cam_y: f32,
    target_cam_y: f32,
    confirming: bool,
    confirm_timer_s: f32,
    show_options: bool,
    show_credits: bool,
    opt_downscroll: bool,
    opt_botplay: bool,
}

impl MainMenuScreenState {
    #[must_use]
    pub fn load(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        downscroll: bool,
        botplay: bool,
    ) -> Self {
        let bg_tex = load_menu_image(assets, gpu, "menuBG");
        let magenta_tex = load_menu_image(assets, gpu, "menuBGMagenta");

        let specs = [
            (MainMenuChoice::StoryMode, "storymode", "mainmenu/storymode", "storymode idle", "storymode selected"),
            (MainMenuChoice::Freeplay, "freeplay", "mainmenu/freeplay", "freeplay idle", "freeplay selected"),
            (MainMenuChoice::Options, "options", "mainmenu/options", "options idle", "options selected"),
            (MainMenuChoice::Credits, "credits", "mainmenu/credits", "credits idle", "credits selected"),
        ];

        let spacing = 152.0;
        let top = (VIRTUAL_HEIGHT - spacing * ((specs.len() - 1) as f32)) * 0.5;

        let mut items = Vec::with_capacity(specs.len());
        for (idx, (choice, label, key, idle_pfx, sel_pfx)) in specs.into_iter().enumerate() {
            let loaded = load_menu_sparrow(assets, gpu, key);
            let (tex, idle_frames, selected_frames) = match loaded {
                Some((t, sz, atlas)) => (
                    Some((t, sz)),
                    atlas.frames_by_prefix(idle_pfx).cloned().collect(),
                    atlas.frames_by_prefix(sel_pfx).cloned().collect(),
                ),
                None => (None, Vec::new(), Vec::new()),
            };
            items.push(MenuItemEntry {
                choice,
                label,
                tex,
                idle_frames,
                selected_frames,
                base_y: top + spacing * (idx as f32),
            });
        }

        let initial_cam_y = items.first().map_or(VIRTUAL_HEIGHT * 0.5, |i| i.base_y);
        Self {
            white_tex: gpu.white_texture(),
            bg_tex,
            magenta_tex,
            items,
            selected: 0,
            anim_time_s: 0.0,
            cam_y: initial_cam_y,
            target_cam_y: initial_cam_y,
            confirming: false,
            confirm_timer_s: 0.0,
            show_options: false,
            show_credits: false,
            opt_downscroll: downscroll,
            opt_botplay: botplay,
        }
    }

    pub fn sync_gameplay_prefs(&mut self, downscroll: bool, botplay: bool) {
        self.opt_downscroll = downscroll;
        self.opt_botplay = botplay;
    }

    #[must_use]
    pub fn downscroll(&self) -> bool {
        self.opt_downscroll
    }

    #[must_use]
    pub fn botplay(&self) -> bool {
        self.opt_botplay
    }

    pub fn reset_for_enter(&mut self) {
        self.confirming = false;
        self.confirm_timer_s = 0.0;
        self.show_options = false;
        self.show_credits = false;
        if let Some(item) = self.items.get(self.selected) {
            self.target_cam_y = item.base_y;
            self.cam_y = item.base_y;
        }
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        let dt = elapsed_s.clamp(0.0, 0.1);
        self.anim_time_s += dt;
        let lerp_t = 1.0 - (1.0 - 0.06_f32).powf(dt * 60.0);
        self.cam_y += (self.target_cam_y - self.cam_y) * lerp_t;

        if self.show_credits {
            if input.back_pressed || input.accept_pressed {
                self.show_credits = false;
                return MenuCommand::PlaySfx("cancelMenu");
            }
            return MenuCommand::None;
        }

        if self.show_options {
            if input.back_pressed {
                self.show_options = false;
                return MenuCommand::PlaySfx("cancelMenu");
            }
            if input.up_pressed || input.down_pressed {
                self.opt_downscroll = !self.opt_downscroll;
                return MenuCommand::PlaySfx("scrollMenu");
            }
            if input.left_pressed || input.right_pressed || input.accept_pressed {
                self.opt_botplay = !self.opt_botplay;
                return MenuCommand::PlaySfx("scrollMenu");
            }
            return MenuCommand::None;
        }

        if self.confirming {
            self.confirm_timer_s += dt;
            if self.confirm_timer_s >= 0.75 {
                self.confirming = false;
                let choice = self
                    .items
                    .get(self.selected)
                    .map_or(MainMenuChoice::StoryMode, |i| i.choice);
                return match choice {
                    MainMenuChoice::StoryMode => MenuCommand::EnterStoryMenu,
                    MainMenuChoice::Freeplay => MenuCommand::EnterFreeplay,
                    MainMenuChoice::Options => {
                        self.show_options = true;
                        MenuCommand::None
                    }
                    MainMenuChoice::Credits => {
                        self.show_credits = true;
                        MenuCommand::None
                    }
                };
            }
            return MenuCommand::None;
        }

        if input.back_pressed {
            return MenuCommand::EnterTitle;
        }

        if input.up_pressed && !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
            self.target_cam_y = self.items[self.selected].base_y;
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.down_pressed && !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
            self.target_cam_y = self.items[self.selected].base_y;
            return MenuCommand::PlaySfx("scrollMenu");
        }

        if input.accept_pressed {
            self.confirming = true;
            self.confirm_timer_s = 0.0;
            return MenuCommand::PlaySfx("confirmMenu");
        }

        MenuCommand::None
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        let flicker_magenta = self.confirming && (((self.confirm_timer_s * 12.0) as i32) & 1 == 1);
        let bg = if flicker_magenta {
            self.magenta_tex.or(self.bg_tex)
        } else {
            self.bg_tex
        };
        if let Some((tex, sz)) = bg {
            let draw_w = VIRTUAL_WIDTH * 1.2;
            let draw_h = sz[1] * (draw_w / sz[0].max(1.0));
            let y_shift = (self.cam_y - VIRTUAL_HEIGHT * 0.5) * 0.17;
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new(
                    [(VIRTUAL_WIDTH - draw_w) * 0.5, (VIRTUAL_HEIGHT - draw_h) * 0.5 - y_shift],
                    [draw_w, draw_h],
                ),
            );
        }

        let item_scroll_y = (self.cam_y - VIRTUAL_HEIGHT * 0.5) * 0.4;
        for (idx, item) in self.items.iter().enumerate() {
            let is_sel = idx == self.selected;
            if self.confirming && !is_sel {
                continue;
            }
            if is_sel && self.confirming && (((self.confirm_timer_s * 16.0) as i32) & 1 == 1) {
                continue;
            }
            if let Some((tex, sz)) = item.tex {
                let frames = if is_sel {
                    &item.selected_frames
                } else {
                    &item.idle_frames
                };
                if let Some(frame) = frames.get(
                    ((self.anim_time_s * 24.0).floor() as usize) % frames.len().max(1),
                ) {
                    let src = frame.source_size();
                    let pos = [
                        (VIRTUAL_WIDTH - src[0]) * 0.5,
                        item.base_y - src[1] * 0.5 - item_scroll_y,
                    ];
                    draw_menu_sparrow_frame(
                        batcher,
                        tex,
                        sz,
                        frame,
                        pos,
                        [1.0, 1.0],
                        false,
                        [1.0, 1.0, 1.0, 1.0],
                        [0.0, 0.0, 0.0],
                        FilterMode::Linear,
                    );
                    continue;
                }
            }
            font.draw_str(
                batcher,
                font_tex,
                item.label,
                VIRTUAL_WIDTH * 0.5,
                item.base_y - 16.0 - item_scroll_y,
                &TextStyle {
                    scale: if is_sel { 1.6 } else { 1.2 },
                    color: if is_sel {
                        [1.0, 0.92, 0.25, 1.0]
                    } else {
                        [1.0, 1.0, 1.0, 0.85]
                    },
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }

        font.draw_str(
            batcher,
            font_tex,
            "fnfrs V-Slice Rust Port",
            14.0,
            VIRTUAL_HEIGHT - 30.0,
            &TextStyle {
                scale: 0.72,
                color: [1.0, 1.0, 1.0, 0.85],
                align: TextAlign::Left,
                ..Default::default()
            },
        );

        if self.show_options {
            self.draw_options_overlay(batcher, font, font_tex);
        } else if self.show_credits {
            self.draw_credits_overlay(batcher, font, font_tex);
        }
    }

    fn draw_options_overlay(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, 0.72]),
        );
        font.draw_str(
            batcher,
            font_tex,
            "OPTIONS",
            VIRTUAL_WIDTH * 0.5,
            150.0,
            &TextStyle {
                scale: 1.5,
                color: [1.0, 0.92, 0.25, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
        );
        let ds_label = if self.opt_downscroll {
            "Up/Down: Downscroll [ON]"
        } else {
            "Up/Down: Downscroll [OFF]"
        };
        let bp_label = if self.opt_botplay {
            "Left/Right/Enter: Botplay [ON]"
        } else {
            "Left/Right/Enter: Botplay [OFF]"
        };
        font.draw_str(
            batcher,
            font_tex,
            ds_label,
            VIRTUAL_WIDTH * 0.5,
            290.0,
            &TextStyle {
                scale: 1.1,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
        );
        font.draw_str(
            batcher,
            font_tex,
            bp_label,
            VIRTUAL_WIDTH * 0.5,
            360.0,
            &TextStyle {
                scale: 1.1,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
        );
        font.draw_str(
            batcher,
            font_tex,
            "Esc / Backspace: Back",
            VIRTUAL_WIDTH * 0.5,
            510.0,
            &TextStyle {
                scale: 0.85,
                color: [0.85, 0.85, 0.9, 0.85],
                align: TextAlign::Center,
                ..Default::default()
            },
        );
    }

    fn draw_credits_overlay(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, 0.72]),
        );
        let lines = [
            ("CREDITS", [1.0, 0.92, 0.25, 1.0], 1.5, 150.0),
            ("Friday Night Funkin' V-Slice", [1.0, 1.0, 1.0, 1.0], 1.1, 270.0),
            ("The Funkin' Crew Inc.", [0.9, 0.8, 0.95, 1.0], 1.0, 330.0),
            ("fnfrs Native Rust + wgpu + cpal Port", [0.65, 0.92, 1.0, 1.0], 1.0, 410.0),
            ("Press Enter or Esc to Return", [0.85, 0.85, 0.9, 0.85], 0.85, 530.0),
        ];
        for (txt, color, scale, y) in lines {
            font.draw_str(
                batcher,
                font_tex,
                txt,
                VIRTUAL_WIDTH * 0.5,
                y,
                &TextStyle {
                    scale,
                    color,
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }
    }
}
