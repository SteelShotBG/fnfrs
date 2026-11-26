//! V-Slice `F3` Debug Display overlay (`FunkinDebugDisplay.hx`).

use std::fmt::Write as _;

use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign,
    TextStyle, TextureHandle,
};

/// Live snapshot passed to [`FunkinDebugDisplay::draw`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugSnapshot<'a> {
    pub screen_label: &'a str,
    pub song_pos_ms: f64,
    pub song_duration_ms: f64,
    pub beat: i32,
    pub step: i32,
    pub bpm: f64,
    pub cam_pos: [f32; 2],
    pub cam_zoom: f32,
    pub sprite_count: usize,
    pub texture_count: usize,
    pub audio_rate_hz: u32,
    pub is_botplay: bool,
    pub is_downscroll: bool,
}

/// Zero-allocation `F3`-toggled FPS & diagnostic HUD overlay.
#[derive(Debug)]
pub struct FunkinDebugDisplay {
    pub visible: bool,
    fps: f32,
    frame_ms: f32,
    frame_accum_s: f32,
    frame_accum_count: u32,
    line1: StackTextBuffer<128>,
    line2: StackTextBuffer<128>,
    line3: StackTextBuffer<128>,
}

impl Default for FunkinDebugDisplay {
    fn default() -> Self {
        Self::new(false)
    }
}

impl FunkinDebugDisplay {
    #[must_use]
    pub fn new(visible: bool) -> Self {
        Self {
            visible,
            fps: 60.0,
            frame_ms: 16.67,
            frame_accum_s: 0.0,
            frame_accum_count: 0,
            line1: StackTextBuffer::new(),
            line2: StackTextBuffer::new(),
            line3: StackTextBuffer::new(),
        }
    }

    pub fn toggle(&mut self) -> bool {
        self.visible = !self.visible;
        self.visible
    }

    /// Accumulates frame timings cleanly (updates readout every 0.25s).
    pub fn update(&mut self, elapsed_s: f32) {
        let dt = elapsed_s.max(0.0);
        self.frame_accum_s += dt;
        self.frame_accum_count += 1;
        if self.frame_accum_s >= 0.25 && self.frame_accum_count > 0 {
            let avg = self.frame_accum_s / (self.frame_accum_count as f32);
            self.frame_ms = avg * 1000.0;
            self.fps = if avg > 1e-5 { 1.0 / avg } else { 0.0 };
            self.frame_accum_s = 0.0;
            self.frame_accum_count = 0;
        }
    }

    /// Renders compact debug info panel at top-left when `visible == true`. Zero allocations.
    pub fn draw(
        &mut self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
        snap: &DebugSnapshot<'_>,
    ) {
        if !self.visible {
            return;
        }

        self.line1.clear();
        let _ = write!(
            self.line1,
            "FPS: {:.0} ({:.2}ms) | Tex: {} | Sprites: {} | Audio: {}Hz",
            self.fps, self.frame_ms, snap.texture_count, snap.sprite_count, snap.audio_rate_hz
        );

        self.line2.clear();
        let _ = write!(
            self.line2,
            "{} | {:.2}s/{:.2}s | Beat: {} Step: {} | BPM: {:.1}",
            snap.screen_label,
            snap.song_pos_ms / 1000.0,
            snap.song_duration_ms / 1000.0,
            snap.beat,
            snap.step,
            snap.bpm
        );

        self.line3.clear();
        let _ = write!(
            self.line3,
            "Cam: ({:.1}, {:.1}) Zoom: {:.2} | Bot: {} | Downscroll: {}",
            snap.cam_pos[0],
            snap.cam_pos[1],
            snap.cam_zoom,
            if snap.is_botplay { "ON" } else { "OFF" },
            if snap.is_downscroll { "ON" } else { "OFF" }
        );

        let scale = if font.line_height() <= 10.0 { 1.5 } else { 0.85 };
        let line_h = if font.line_height() <= 10.0 { 16.0 } else { 20.0 };
        let box_w = 560.0;
        let box_h = line_h * 3.0 + 16.0;

        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([8.0, 8.0], [box_w, box_h]).with_color([0.0, 0.0, 0.0, 0.72]),
        );

        let style = TextStyle {
            scale,
            color: [0.92, 0.96, 1.0, 1.0],
            align: TextAlign::Left,
            ..Default::default()
        };
        for (i, buf) in [&self.line1, &self.line2, &self.line3].into_iter().enumerate() {
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                buf.as_bytes(),
                [16.0, 14.0 + (i as f32) * line_h],
                &style,
                [0.0, 0.0, 0.0, 1.0],
            );
        }
    }
}
