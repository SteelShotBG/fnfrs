//! V-Slice Credits State (`CreditsState.hx`).
//!
//! Scrolls credits vertically bottom-to-top using `vcr-bmp` outlined text over a
//! subtle lavender `menuDesat` backdrop, matching official controls:
//! - Default scroll: `100 px/s`
//! - Hold `Enter` / `Space`: `400 px/s`
//! - Hold `Shift`: pause scroll (`0 px/s`)
//! - `Esc` / `Backspace` or end of credits: return to `MainMenu`

use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextAlign,
    TextStyle, TextureHandle,
};
use serde::Deserialize;

use super::{load_menu_image, MenuCommand, MenuInputState};

const SCREEN_PAD: f32 = 24.0;
const CREDITS_FONT_SIZE: f32 = 24.0;
const CREDITS_HEADER_SCALE: f32 = 2.0;
const CREDITS_BODY_SCALE: f32 = 1.5;
const CREDITS_BASE_SPEED: f32 = 100.0;
const CREDITS_FAST_SPEED: f32 = CREDITS_BASE_SPEED * 4.0;
const LAVENDER_COLOR: [f32; 4] = [0.71, 0.49, 0.86, 0.1];

#[derive(Debug, Deserialize)]
struct CreditsFile {
    #[serde(default)]
    entries: Vec<CreditsFileRole>,
}

#[derive(Debug, Deserialize)]
struct CreditsFileRole {
    header: Option<String>,
    #[serde(default)]
    body: Vec<CreditsFileMember>,
}

#[derive(Debug, Deserialize)]
struct CreditsFileMember {
    line: String,
}

#[derive(Debug)]
struct RenderedCreditLine {
    text: String,
    y_offset: f32,
    is_header: bool,
}

#[derive(Debug)]
pub struct CreditsScreenState {
    bg_tex: Option<(TextureHandle, [f32; 2])>,
    lines: Vec<RenderedCreditLine>,
    scroll_y: f32,
    total_height: f32,
    exiting: bool,
}

impl CreditsScreenState {
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext) -> Self {
        let bg_tex = load_menu_image(assets, gpu, "menuDesat");
        let roles = load_credits_roles(assets);

        let mut lines = Vec::with_capacity(64);
        let mut cursor_y = 0.0_f32;
        for role in roles {
            if let Some(header) = role.header {
                lines.push(RenderedCreditLine {
                    text: header,
                    y_offset: cursor_y,
                    is_header: true,
                });
                cursor_y += 32.0 + 32.0;
            }
            for member in role.body {
                lines.push(RenderedCreditLine {
                    text: member.line,
                    y_offset: cursor_y,
                    is_header: false,
                });
                cursor_y += CREDITS_FONT_SIZE;
            }
            cursor_y += CREDITS_FONT_SIZE * 2.5;
        }

        Self {
            bg_tex,
            lines,
            scroll_y: VIRTUAL_HEIGHT,
            total_height: cursor_y,
            exiting: false,
        }
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        if self.exiting {
            return MenuCommand::None;
        }
        let dt = elapsed_s.clamp(0.0, 0.1);
        let speed = if input.accept_held {
            CREDITS_FAST_SPEED
        } else if input.shift_held {
            0.0
        } else {
            CREDITS_BASE_SPEED
        };
        self.scroll_y -= speed * dt;

        let ended = self.scroll_y + self.total_height <= 0.0;
        if input.back_pressed || ended {
            self.exiting = true;
            return MenuCommand::ExitCreditsToMainMenu;
        }
        MenuCommand::None
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        if let Some((tex, sz)) = self.bg_tex {
            let draw_w = VIRTUAL_WIDTH;
            let draw_h = sz[1] * (draw_w / sz[0].max(1.0));
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([0.0, 0.0], [draw_w, draw_h]).with_color(LAVENDER_COLOR),
            );
        }

        for line in &self.lines {
            let y = self.scroll_y + line.y_offset;
            if y < -64.0 || y > VIRTUAL_HEIGHT + 32.0 {
                continue;
            }
            let style = TextStyle {
                scale: if line.is_header {
                    CREDITS_HEADER_SCALE
                } else {
                    CREDITS_BODY_SCALE
                },
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            };
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                line.text.as_bytes(),
                [SCREEN_PAD, y],
                &style,
                [0.0, 0.0, 0.0, 1.0],
            );
        }
    }
}

fn load_credits_roles(assets: &AssetLocator) -> Vec<CreditsFileRole> {
    let candidates = [
        assets.root().join("data").join("credits.json"),
        assets.root().join("exclude").join("data").join("credits.json"),
    ];
    for path in candidates {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<CreditsFile>(&raw) {
                if !parsed.entries.is_empty() {
                    return parsed.entries;
                }
            }
        }
    }

    // Official Funkin' Crew fallback (matching CreditsDataHandler.getFallback + V-Slice crew).
    vec![
        CreditsFileRole {
            header: Some("Friday Night Funkin'".to_owned()),
            body: vec![
                CreditsFileMember { line: "ninjamuffin99 - Programmer".to_owned() },
                CreditsFileMember { line: "PhantomArcade - Animator & Director".to_owned() },
                CreditsFileMember { line: "Kawai Sprite - Musician".to_owned() },
                CreditsFileMember { line: "evilsk8r - Artist".to_owned() },
            ],
        },
        CreditsFileRole {
            header: Some("The Funkin' Crew".to_owned()),
            body: vec![
                CreditsFileMember { line: "EliteMasterEric - Lead Programmer".to_owned() },
                CreditsFileMember { line: "MtH - Charting & Programming".to_owned() },
                CreditsFileMember { line: "JohnnyUtah - Voice Acting & Art".to_owned() },
                CreditsFileMember { line: "Jenny - Illustration & Animation".to_owned() },
                CreditsFileMember { line: "Bassetfilms - Music & Sound".to_owned() },
                CreditsFileMember { line: "Saster - Music".to_owned() },
                CreditsFileMember { line: "Saruky - Music".to_owned() },
            ],
        },
        CreditsFileRole {
            header: Some("fnfrs Rust Port".to_owned()),
            body: vec![
                CreditsFileMember { line: "Zero-Allocation V-Slice Engine".to_owned() },
                CreditsFileMember { line: "wgpu + winit + cpal + symphonia".to_owned() },
            ],
        },
    ]
}
