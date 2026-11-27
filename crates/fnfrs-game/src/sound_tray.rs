//! V-Slice global volume SoundTray overlay (`FunkinSoundTray.hx`).

use fnfrs_core::constants::VIRTUAL_WIDTH;
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, StackTextBuffer,
    TextAlign, TextStyle, TextureHandle,
};
use std::fmt::Write as _;

/// Slide-down volume tray rendered above all screens when `-`, `+`, or `0` is pressed.
#[derive(Debug)]
pub struct SoundTray {
    white_tex: TextureHandle,
    box_tex: Option<(TextureHandle, [f32; 2])>,
    bar_tex: [Option<(TextureHandle, [f32; 2])>; 10],
    volume: f32,
    muted: bool,
    visible_timer_s: f32,
    lerp_y: f32,
}

impl SoundTray {
    /// Loads `images/soundtray/{volumebox,bars_1..10}.png`.
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext, volume: f32, muted: bool) -> Self {
        let box_tex = upload_ui_png(assets, gpu, "soundtray/volumebox");
        let mut bar_tex = [None; 10];
        for (idx, slot) in bar_tex.iter_mut().enumerate() {
            let key = format!("soundtray/bars_{}", idx + 1);
            *slot = upload_ui_png(assets, gpu, &key);
        }
        let mut tray = Self {
            white_tex: gpu.white_texture(),
            box_tex,
            bar_tex,
            volume: volume.clamp(0.0, 1.0),
            muted,
            visible_timer_s: 0.0,
            lerp_y: -90.0,
        };
        tray.pin_textures(gpu);
        tray
    }

    /// Pins SoundTray textures inside `gpu` so screen transitions never evict them.
    pub fn pin_textures(&self, gpu: &mut GpuContext) {
        if let Some((tex, _)) = self.box_tex {
            gpu.pin_texture(tex);
        }
        for slot in self.bar_tex.iter().flatten() {
            gpu.pin_texture(slot.0);
        }
    }

    /// Re-uploads textures cleanly after `gpu.clear_user_textures()`.
    pub fn reload_textures(&mut self, assets: &AssetLocator, gpu: &mut GpuContext) {
        self.white_tex = gpu.white_texture();
        self.box_tex = upload_ui_png(assets, gpu, "soundtray/volumebox");
        for (idx, slot) in self.bar_tex.iter_mut().enumerate() {
            let key = format!("soundtray/bars_{}", idx + 1);
            *slot = upload_ui_png(assets, gpu, &key);
        }
        self.pin_textures(gpu);
    }

    /// Returns effective gain `0.0..=1.0`.
    #[inline]
    #[must_use]
    pub fn effective_volume(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }

    #[inline]
    #[must_use]
    pub fn raw_volume(&self) -> f32 {
        self.volume
    }

    #[inline]
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Adjusts volume by `delta_steps` tenths (`+1`, `-1`) or toggles mute (`toggle_mute`).
    /// Returns the SFX key name (`"soundtray/Volup"`, `"soundtray/Voldown"`, or `"soundtray/VolMAX"`).
    pub fn adjust(&mut self, delta_steps: i32, toggle_mute: bool) -> &'static str {
        self.visible_timer_s = 1.6;
        if toggle_mute {
            self.muted = !self.muted;
            return if self.muted {
                "soundtray/Voldown"
            } else if self.volume >= 0.99 {
                "soundtray/VolMAX"
            } else {
                "soundtray/Volup"
            };
        }
        self.muted = false;
        let prev_step = (self.volume * 10.0).round() as i32;
        let next_step = (prev_step + delta_steps).clamp(0, 10);
        self.volume = (next_step as f32) * 0.1;
        if delta_steps >= 0 && next_step >= 10 {
            "soundtray/VolMAX"
        } else if delta_steps >= 0 {
            "soundtray/Volup"
        } else {
            "soundtray/Voldown"
        }
    }

    /// Advances tray timer & slide-in/out Y position.
    pub fn update(&mut self, elapsed_s: f32) {
        let dt = elapsed_s.clamp(0.0, 0.1);
        if self.visible_timer_s > 0.0 {
            self.visible_timer_s = (self.visible_timer_s - dt).max(0.0);
        }
        let target_y = if self.visible_timer_s > 0.0 { 16.0 } else { -110.0 };
        let t = 1.0 - (-14.0 * dt).exp();
        self.lerp_y += (target_y - self.lerp_y) * t;
    }

    /// Draws the SoundTray into `batcher` with zero heap allocations.
    pub fn draw(&self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        if self.lerp_y <= -95.0 {
            return;
        }
        let step = if self.muted {
            0usize
        } else {
            ((self.volume * 10.0).round() as usize).clamp(0, 10)
        };
        let scale = 0.6_f32;
        if let Some((box_t, box_sz)) = self.box_tex {
            let w = box_sz[0] * scale;
            let h = box_sz[1] * scale;
            let x = (VIRTUAL_WIDTH - w) * 0.5;
            let y = self.lerp_y;
            batcher.push(
                box_t,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([x, y], [w, h]),
            );
            if step > 0 {
                if let Some((bar_t, bar_sz)) = self.bar_tex[step - 1] {
                    let bw = bar_sz[0] * scale;
                    let bh = bar_sz[1] * scale;
                    batcher.push(
                        bar_t,
                        BlendMode::Normal,
                        FilterMode::Linear,
                        SpriteInstance::new([(VIRTUAL_WIDTH - bw) * 0.5, y + 8.0 * scale], [bw, bh]),
                    );
                }
            }
        } else {
            let w = 240.0;
            let h = 68.0;
            let x = (VIRTUAL_WIDTH - w) * 0.5;
            let y = self.lerp_y;
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([x, y], [w, h]).with_color([0.05, 0.05, 0.08, 0.88]),
            );
            let mut buf = StackTextBuffer::<32>::new();
            let pct = if self.muted { 0 } else { step * 10 };
            let _ = write!(buf, "VOLUME {pct}%");
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                buf.as_bytes(),
                [VIRTUAL_WIDTH * 0.5, y + 18.0],
                &TextStyle {
                    scale: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, 1.0],
            );
        }
    }
}

fn upload_ui_png(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    key: &str,
) -> Option<(TextureHandle, [f32; 2])> {
    let path = assets.image(key);
    let tex = gpu.from_png_path_cached(Some(key), &path).ok()?;
    let (w, h) = gpu.texture_size(tex)?;
    Some((tex, [w as f32, h as f32]))
}
