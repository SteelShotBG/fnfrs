//! V-Slice Options Menu (`OptionsState.hx`: Preferences, Controls, Offset Calibrator).

use std::fmt::Write as _;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::save::SaveOptions;
use fnfrs_render::{
    AtlasFont, AtlasTextStyle, BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher,
    SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle,
};

use super::{load_menu_atlas_font, load_menu_image, MenuCommand, MenuInputState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionsTab {
    CategorySelect,
    Preferences,
    Controls,
}

const CATEGORIES: [&str; 3] = ["PREFERENCES", "CONTROLS", "EXIT"];
const LANE_NAMES: [&str; 4] = ["NOTE LEFT", "NOTE DOWN", "NOTE UP", "NOTE RIGHT"];

/// Interactive Options screen persisted directly into `SaveOptions`.
#[derive(Debug)]
pub struct OptionsScreenState {
    white_tex: TextureHandle,
    bg_tex: Option<(TextureHandle, [f32; 2])>,
    atlas_font: AtlasFont,
    tab: OptionsTab,
    cat_idx: usize,
    pref_idx: usize,
    ctrl_idx: usize,
    rebinding_lane: Option<usize>,
    elapsed_s: f32,
    pub options: SaveOptions,
    pub pending_key_capture: Option<String>,
}

impl OptionsScreenState {
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext, options: SaveOptions) -> Self {
        let bg_tex = load_menu_image(assets, gpu, "menuDesat")
            .or_else(|| load_menu_image(assets, gpu, "menuBG"));
        let atlas_font = load_menu_atlas_font(assets, gpu, true);
        Self {
            white_tex: gpu.white_texture(),
            bg_tex,
            atlas_font,
            tab: OptionsTab::CategorySelect,
            cat_idx: 0,
            pref_idx: 0,
            ctrl_idx: 0,
            rebinding_lane: None,
            elapsed_s: 0.0,
            options,
            pending_key_capture: None,
        }
    }

    pub fn feed_captured_key(&mut self, key_name: &str) -> bool {
        let Some(lane) = self.rebinding_lane else {
            return false;
        };
        if !key_name.eq_ignore_ascii_case("ESCAPE") && lane < 4 {
            self.options.keybinds[lane] = key_name.to_owned();
        }
        self.rebinding_lane = None;
        true
    }

    #[must_use]
    pub fn is_awaiting_rebind(&self) -> bool {
        self.rebinding_lane.is_some()
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        self.elapsed_s += elapsed_s.clamp(0.0, 0.1);
        if self.rebinding_lane.is_some() {
            if input.back_pressed {
                self.rebinding_lane = None;
                return MenuCommand::PlaySfx("cancelMenu");
            }
            return MenuCommand::None;
        }

        match self.tab {
            OptionsTab::CategorySelect => {
                if input.back_pressed {
                    return MenuCommand::EnterMainMenu;
                }
                if input.up_pressed {
                    self.cat_idx = (self.cat_idx + CATEGORIES.len() - 1) % CATEGORIES.len();
                    return MenuCommand::PlaySfx("scrollMenu");
                }
                if input.down_pressed {
                    self.cat_idx = (self.cat_idx + 1) % CATEGORIES.len();
                    return MenuCommand::PlaySfx("scrollMenu");
                }
                if input.accept_pressed {
                    match self.cat_idx {
                        0 => self.tab = OptionsTab::Preferences,
                        1 => self.tab = OptionsTab::Controls,
                        _ => return MenuCommand::EnterMainMenu,
                    }
                    return MenuCommand::PlaySfx("confirmMenu");
                }
            }
            OptionsTab::Preferences => {
                const PREF_COUNT: usize = 8;
                if input.back_pressed {
                    self.tab = OptionsTab::CategorySelect;
                    return MenuCommand::PlaySfx("cancelMenu");
                }
                if input.up_pressed {
                    self.pref_idx = (self.pref_idx + PREF_COUNT - 1) % PREF_COUNT;
                    return MenuCommand::PlaySfx("scrollMenu");
                }
                if input.down_pressed {
                    self.pref_idx = (self.pref_idx + 1) % PREF_COUNT;
                    return MenuCommand::PlaySfx("scrollMenu");
                }
                if self.pref_idx == 7 {
                    // Audio Offset (+/- 5ms per tap, +/- 1ms with shift)
                    let step = if input.shift_held { 1.0 } else { 5.0 };
                    if input.left_pressed {
                        self.options.audio_offset_ms =
                            (self.options.audio_offset_ms - step).clamp(-500.0, 500.0);
                        return MenuCommand::SavePreferencesChanged;
                    }
                    if input.right_pressed {
                        self.options.audio_offset_ms =
                            (self.options.audio_offset_ms + step).clamp(-500.0, 500.0);
                        return MenuCommand::SavePreferencesChanged;
                    }
                } else if input.accept_pressed || input.left_pressed || input.right_pressed {
                    match self.pref_idx {
                        0 => self.options.downscroll = !self.options.downscroll,
                        1 => self.options.botplay = !self.options.botplay,
                        2 => self.options.flashing_lights = !self.options.flashing_lights,
                        3 => self.options.zoom_camera = !self.options.zoom_camera,
                        4 => self.options.naughtyness = !self.options.naughtyness,
                        5 => self.options.auto_pause = !self.options.auto_pause,
                        6 => self.options.debug_display = !self.options.debug_display,
                        _ => {}
                    }
                    return MenuCommand::SavePreferencesChanged;
                }
            }
            OptionsTab::Controls => {
                if input.back_pressed {
                    self.tab = OptionsTab::CategorySelect;
                    return MenuCommand::PlaySfx("cancelMenu");
                }
                if input.up_pressed {
                    self.ctrl_idx = (self.ctrl_idx + 3) % 4;
                    return MenuCommand::PlaySfx("scrollMenu");
                }
                if input.down_pressed {
                    self.ctrl_idx = (self.ctrl_idx + 1) % 4;
                    return MenuCommand::PlaySfx("scrollMenu");
                }
                if input.accept_pressed {
                    self.rebinding_lane = Some(self.ctrl_idx);
                    return MenuCommand::PlaySfx("confirmMenu");
                }
            }
        }
        MenuCommand::None
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        if let Some((tex, sz)) = self.bg_tex {
            let s = (VIRTUAL_WIDTH / sz[0].max(1.0)).max(VIRTUAL_HEIGHT / sz[1].max(1.0));
            let w = sz[0] * s;
            let h = sz[1] * s;
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([(VIRTUAL_WIDTH - w) * 0.5, (VIRTUAL_HEIGHT - h) * 0.5], [w, h])
                    .with_color([0.92, 0.43, 0.51, 1.0]),
            );
        } else {
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([0.14, 0.09, 0.22, 1.0]),
            );
        }

        let title = match self.tab {
            OptionsTab::CategorySelect => "OPTIONS",
            OptionsTab::Preferences => "PREFERENCES",
            OptionsTab::Controls => "CONTROLS",
        };
        if self.atlas_font.is_loaded() {
            self.atlas_font.draw_str(
                batcher,
                title,
                [VIRTUAL_WIDTH * 0.5, 44.0],
                &AtlasTextStyle {
                    scale: 0.9,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    time_s: self.elapsed_s,
                    animated: true,
                    ..Default::default()
                },
            );
        }

        let mut buf = StackTextBuffer::<96>::new();
        match self.tab {
            OptionsTab::CategorySelect => {
                for (i, label) in CATEGORIES.iter().enumerate() {
                    let is_sel = i == self.cat_idx;
                    let y = 210.0 + (i as f32) * 96.0;
                    if self.atlas_font.is_loaded() {
                        self.atlas_font.draw_str(
                            batcher,
                            label,
                            [VIRTUAL_WIDTH * 0.5, y],
                            &AtlasTextStyle {
                                scale: if is_sel { 0.95 } else { 0.8 },
                                color: if is_sel { [1.0, 0.94, 0.3, 1.0] } else { [1.0, 1.0, 1.0, 0.72] },
                                align: TextAlign::Center,
                                time_s: self.elapsed_s,
                                animated: is_sel,
                                ..Default::default()
                            },
                        );
                    } else {
                        font.draw_bytes_outlined(
                            batcher,
                            font_tex,
                            label.as_bytes(),
                            [VIRTUAL_WIDTH * 0.5, y],
                            &TextStyle {
                                scale: 1.4,
                                color: [1.0, 1.0, 1.0, 1.0],
                                align: TextAlign::Center,
                                ..Default::default()
                            },
                            [0.0, 0.0, 0.0, 1.0],
                        );
                    }
                }
            }
            OptionsTab::Preferences => {
                let rows: [(&str, bool, Option<f32>); 8] = [
                    ("DOWNSCROLL", self.options.downscroll, None),
                    ("BOTPLAY", self.options.botplay, None),
                    ("FLASHING LIGHTS", self.options.flashing_lights, None),
                    ("CAMERA ZOOMING", self.options.zoom_camera, None),
                    ("NAUGHTYNESS", self.options.naughtyness, None),
                    ("AUTO PAUSE", self.options.auto_pause, None),
                    ("DEBUG DISPLAY (F6)", self.options.debug_display, None),
                    ("AUDIO OFFSET MS", false, Some(self.options.audio_offset_ms)),
                ];
                for (i, (label, val, opt_ms)) in rows.iter().enumerate() {
                    let is_sel = i == self.pref_idx;
                    let y = 148.0 + (i as f32) * 62.0;
                    buf.clear();
                    if let Some(ms) = opt_ms {
                        let _ = write!(buf, "{label}: {ms:.0} MS");
                    } else {
                        let _ = write!(buf, "{label}: {}", if *val { "ON" } else { "OFF" });
                    }
                    let color = if is_sel {
                        [1.0, 0.94, 0.3, 1.0]
                    } else {
                        [1.0, 1.0, 1.0, 0.78]
                    };
                    if self.atlas_font.is_loaded() {
                        self.atlas_font.draw_bytes(
                            batcher,
                            buf.as_bytes(),
                            [120.0, y],
                            &AtlasTextStyle {
                                scale: if is_sel { 0.62 } else { 0.55 },
                                color,
                                align: TextAlign::Left,
                                time_s: self.elapsed_s,
                                animated: is_sel,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            OptionsTab::Controls => {
                for (i, lane_name) in LANE_NAMES.iter().enumerate() {
                    let is_sel = i == self.ctrl_idx;
                    let y = 210.0 + (i as f32) * 86.0;
                    buf.clear();
                    if self.rebinding_lane == Some(i) {
                        let _ = write!(buf, "{lane_name}: PRESS ANY KEY...");
                    } else {
                        let _ = write!(buf, "{lane_name}: {}", self.options.keybinds[i]);
                    }
                    if self.atlas_font.is_loaded() {
                        self.atlas_font.draw_bytes(
                            batcher,
                            buf.as_bytes(),
                            [140.0, y],
                            &AtlasTextStyle {
                                scale: if is_sel { 0.72 } else { 0.64 },
                                color: if is_sel { [1.0, 0.94, 0.3, 1.0] } else { [1.0, 1.0, 1.0, 0.78] },
                                align: TextAlign::Left,
                                time_s: self.elapsed_s,
                                animated: is_sel,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }
    }
}
