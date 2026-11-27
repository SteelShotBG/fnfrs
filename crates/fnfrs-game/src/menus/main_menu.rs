//! V-Slice Main Menu (`MainMenuState.hx` + `AtlasMenuList.hx` + `MenuList.hx`).

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AtlasFrame, BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance,
    TextAlign, TextStyle, TextureHandle,
};

use crate::camera::FlxEase;

use super::{draw_menu_sparrow_frame, load_menu_image, load_menu_sparrow, MenuCommand, MenuInputState};

pub const MERCH_URL: &str = "https://needlejuicerecords.com/en-ca/pages/friday-night-funkin";
const VERSION_WATERMARK: &str = concat!("v", env!("CARGO_PKG_VERSION"));

const ITEM_SPACING: f32 = 160.0;
const BG_WIDTH_FACTOR: f32 = 1.2;
const BG_SCROLL_FACTOR_Y: f32 = 0.17;
const ITEM_SCROLL_FACTOR_Y: f32 = 0.4;
const CAM_FOLLOW_LERP: f32 = 0.06;
const MAGENTA_FLICKER_DURATION_S: f32 = 1.1;
const MAGENTA_FLICKER_INTERVAL_S: f32 = 0.15;
const ITEM_FLICKER_DURATION_S: f32 = 1.0;
const ITEM_FLICKER_INTERVAL_S: f32 = 0.06;
const EXIT_FADE_DURATION_S: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainMenuChoice {
    StoryMode,
    Freeplay,
    Merch,
    Options,
    Credits,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MainMenuPhase {
    Idle,
    Confirming { timer_s: f32 },
    ExitingFade { timer_s: f32 },
    Exited,
}

#[derive(Debug)]
struct MenuItemEntry {
    choice: MainMenuChoice,
    label: &'static str,
    fire_instantly: bool,
    tex: Option<(TextureHandle, [f32; 2])>,
    idle_frames: Vec<AtlasFrame>,
    selected_frames: Vec<AtlasFrame>,
    base_y: f32,
}

#[derive(Debug)]
pub struct MainMenuScreenState {
    bg_tex: Option<(TextureHandle, [f32; 2])>,
    magenta_tex: Option<(TextureHandle, [f32; 2])>,
    items: Vec<MenuItemEntry>,
    selected: usize,
    flashing_lights: bool,
    anim_time_s: f32,
    cam_y: f32,
    target_cam_y: f32,
    magenta_flicker_s: f32,
    phase: MainMenuPhase,
}

impl MainMenuScreenState {
    #[must_use]
    pub fn load(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        flashing_lights: bool,
        remembered_selected: usize,
    ) -> Self {
        let bg_tex = load_menu_image(assets, gpu, "menuBG");
        let magenta_tex = load_menu_image(assets, gpu, "menuBGMagenta");

        let specs = [
            (
                MainMenuChoice::StoryMode,
                "storymode",
                "mainmenu/storymode",
                "storymode idle",
                "storymode selected",
                false,
            ),
            (
                MainMenuChoice::Freeplay,
                "freeplay",
                "mainmenu/freeplay",
                "freeplay idle",
                "freeplay selected",
                false,
            ),
            (
                MainMenuChoice::Merch,
                "merch",
                "mainmenu/merch",
                "merch idle",
                "merch selected",
                true,
            ),
            (
                MainMenuChoice::Options,
                "options",
                "mainmenu/options",
                "options idle",
                "options selected",
                false,
            ),
            (
                MainMenuChoice::Credits,
                "credits",
                "mainmenu/credits",
                "credits idle",
                "credits selected",
                false,
            ),
        ];

        let top = (VIRTUAL_HEIGHT - ITEM_SPACING * (specs.len().saturating_sub(1) as f32)) * 0.5;

        let mut items = Vec::with_capacity(specs.len());
        for (idx, (choice, label, key, idle_pfx, sel_pfx, fire_instantly)) in
            specs.into_iter().enumerate()
        {
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
                fire_instantly,
                tex,
                idle_frames,
                selected_frames,
                base_y: top + ITEM_SPACING * (idx as f32),
            });
        }

        let selected = if items.is_empty() {
            0
        } else {
            remembered_selected.min(items.len() - 1)
        };
        let initial_cam_y = items
            .get(selected)
            .map_or(VIRTUAL_HEIGHT * 0.5, |item| Self::item_cam_y(item, true));

        Self {
            bg_tex,
            magenta_tex,
            items,
            selected,
            flashing_lights,
            anim_time_s: 0.0,
            cam_y: initial_cam_y,
            target_cam_y: initial_cam_y,
            magenta_flicker_s: -1.0,
            phase: MainMenuPhase::Idle,
        }
    }

    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_flashing_lights(&mut self, flashing_lights: bool) {
        self.flashing_lights = flashing_lights;
    }

    pub fn reset_for_enter(&mut self, remembered_selected: usize, flashing_lights: bool) {
        self.flashing_lights = flashing_lights;
        if !self.items.is_empty() {
            self.selected = remembered_selected.min(self.items.len() - 1);
        }
        self.magenta_flicker_s = -1.0;
        self.phase = MainMenuPhase::Idle;
        if let Some(item) = self.items.get(self.selected) {
            let y = Self::item_cam_y(item, true);
            self.target_cam_y = y;
            self.cam_y = y;
        }
    }

    fn item_cam_y(item: &MenuItemEntry, is_selected: bool) -> f32 {
        let frames = if is_selected && !item.selected_frames.is_empty() {
            &item.selected_frames
        } else {
            &item.idle_frames
        };
        let half_h = frames.first().map_or(61.0, |f| f.source_size()[1] * 0.5);
        item.base_y + half_h
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        let dt = elapsed_s.clamp(0.0, 0.1);
        self.anim_time_s += dt;
        let lerp_t = 1.0 - (1.0 - CAM_FOLLOW_LERP).powf(dt * 60.0);
        self.cam_y += (self.target_cam_y - self.cam_y) * lerp_t;

        if self.magenta_flicker_s >= 0.0 {
            self.magenta_flicker_s += dt;
            if self.magenta_flicker_s >= MAGENTA_FLICKER_DURATION_S {
                self.magenta_flicker_s = -1.0;
            }
        }

        match self.phase {
            MainMenuPhase::Confirming { mut timer_s } => {
                timer_s += dt;
                if timer_s >= ITEM_FLICKER_DURATION_S {
                    let choice = self
                        .items
                        .get(self.selected)
                        .map_or(MainMenuChoice::StoryMode, |i| i.choice);
                    if choice == MainMenuChoice::Freeplay {
                        self.phase = MainMenuPhase::Exited;
                        return MenuCommand::EnterFreeplay;
                    }
                    self.phase = MainMenuPhase::ExitingFade { timer_s: 0.0 };
                } else {
                    self.phase = MainMenuPhase::Confirming { timer_s };
                }
                return MenuCommand::None;
            }
            MainMenuPhase::ExitingFade { mut timer_s } => {
                timer_s += dt;
                if timer_s >= EXIT_FADE_DURATION_S {
                    self.phase = MainMenuPhase::Exited;
                    let choice = self
                        .items
                        .get(self.selected)
                        .map_or(MainMenuChoice::StoryMode, |i| i.choice);
                    return match choice {
                        MainMenuChoice::StoryMode => MenuCommand::EnterStoryMenu,
                        MainMenuChoice::Freeplay => MenuCommand::EnterFreeplay,
                        MainMenuChoice::Merch => MenuCommand::OpenUrl(MERCH_URL),
                        MainMenuChoice::Options => MenuCommand::EnterOptions,
                        MainMenuChoice::Credits => MenuCommand::EnterCredits,
                    };
                }
                self.phase = MainMenuPhase::ExitingFade { timer_s };
                return MenuCommand::None;
            }
            MainMenuPhase::Exited => return MenuCommand::None,
            MainMenuPhase::Idle => {}
        }

        if input.back_pressed {
            self.phase = MainMenuPhase::Exited;
            return MenuCommand::CancelToTitle;
        }

        if input.up_pressed && !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
            self.target_cam_y = Self::item_cam_y(&self.items[self.selected], true);
            return MenuCommand::PlaySfxVol("scrollMenu", 0.4);
        }
        if input.down_pressed && !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
            self.target_cam_y = Self::item_cam_y(&self.items[self.selected], true);
            return MenuCommand::PlaySfxVol("scrollMenu", 0.4);
        }

        if input.accept_pressed && !self.items.is_empty() {
            self.magenta_flicker_s = 0.0;
            if self.items[self.selected].fire_instantly {
                return MenuCommand::OpenUrl(MERCH_URL);
            }
            self.phase = MainMenuPhase::Confirming { timer_s: 0.0 };
            return MenuCommand::PlaySfxVol("confirmMenu", 1.0);
        }

        MenuCommand::None
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        let cam_scroll_y = self.cam_y - VIRTUAL_HEIGHT * 0.5;
        let show_magenta = self.flashing_lights
            && self.magenta_flicker_s >= 0.0
            && (((self.magenta_flicker_s / MAGENTA_FLICKER_INTERVAL_S) as i32) & 1 == 0);
        let bg = if show_magenta {
            self.magenta_tex.or(self.bg_tex)
        } else {
            self.bg_tex
        };
        if let Some((tex, sz)) = bg {
            let draw_w = (VIRTUAL_WIDTH * BG_WIDTH_FACTOR).floor();
            let draw_h = sz[1] * (draw_w / sz[0].max(1.0));
            let y_shift = cam_scroll_y * BG_SCROLL_FACTOR_Y;
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new(
                    [
                        (VIRTUAL_WIDTH - draw_w) * 0.5,
                        (VIRTUAL_HEIGHT - draw_h) * 0.5 - y_shift,
                    ],
                    [draw_w, draw_h],
                ),
            );
        }

        let item_scroll_y = cam_scroll_y * ITEM_SCROLL_FACTOR_Y;
        for (idx, item) in self.items.iter().enumerate() {
            let is_sel = idx == self.selected;
            let (visible, alpha) = match self.phase {
                MainMenuPhase::Idle => (true, 1.0),
                MainMenuPhase::Confirming { timer_s } => {
                    if is_sel {
                        ((((timer_s / ITEM_FLICKER_INTERVAL_S) as i32) & 1) == 0, 1.0)
                    } else {
                        (true, 1.0)
                    }
                }
                MainMenuPhase::ExitingFade { timer_s } => {
                    if is_sel {
                        (false, 0.0)
                    } else {
                        let t = (timer_s / EXIT_FADE_DURATION_S).clamp(0.0, 1.0);
                        (true, (1.0 - FlxEase::QuadOut.eval(t)).max(0.0))
                    }
                }
                MainMenuPhase::Exited => {
                    let is_freeplay = self
                        .items
                        .get(self.selected)
                        .is_some_and(|s| s.choice == MainMenuChoice::Freeplay);
                    if is_freeplay {
                        (true, 1.0)
                    } else {
                        (false, 0.0)
                    }
                }
            };
            if !visible || alpha <= 0.001 {
                continue;
            }

            if let Some((tex, sz)) = item.tex {
                let frames = if is_sel && !item.selected_frames.is_empty() {
                    &item.selected_frames
                } else {
                    &item.idle_frames
                };
                if let Some(frame) = frames
                    .get(((self.anim_time_s * 24.0).floor() as usize) % frames.len().max(1))
                {
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
                        [1.0, 1.0, 1.0, alpha],
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
                        [1.0, 0.92, 0.25, alpha]
                    } else {
                        [1.0, 1.0, 1.0, 0.85 * alpha]
                    },
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            VERSION_WATERMARK.as_bytes(),
            [4.0, VIRTUAL_HEIGHT - 18.0],
            &TextStyle {
                scale: 1.0,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );
    }
}
