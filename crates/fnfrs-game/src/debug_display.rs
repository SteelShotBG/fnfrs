//! V-Slice `F6` Debug Display overlay (`FunkinDebugDisplay.hx` + `FunkinStatsGraph.hx`).

use std::fmt::Write as _;

use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign,
    TextStyle, TextureHandle,
};

const UPDATE_DELAY_S: f32 = 0.1;
const INNER_RECT_DIFF: f32 = 3.0;
const OUTER_RECT_DIMENSIONS: [f32; 2] = [234.0, 201.0];
const OTHERS_OFFSET: f32 = 8.0;
const PANEL_POS: [f32; 2] = [10.0, 10.0];
const GRAPH_HISTORY_MAX: usize = 100;
const GRAPH_HEIGHT: f32 = 25.0;
const BG_ALPHA: f32 = 0.5;
const BORDER_COLOR: [f32; 4] = [0x3D as f32 / 255.0, 0x3F as f32 / 255.0, 0x41 as f32 / 255.0, BG_ALPHA];
const INNER_COLOR: [f32; 4] = [0x2C as f32 / 255.0, 0x2F as f32 / 255.0, 0x30 as f32 / 255.0, BG_ALPHA];

#[cfg(target_os = "windows")]
mod win_mem {
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            ppsmem_counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    pub fn task_memory_bytes() -> Option<f64> {
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: `counters.cb` matches the struct size and `GetCurrentProcess()` is a pseudo-handle.
        let ok = unsafe {
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb)
        };
        if ok != 0 {
            Some(counters.working_set_size as f64)
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod win_mem {
    pub fn task_memory_bytes() -> Option<f64> {
        None
    }
}

/// Active mode of the `F6` debug overlay (`Main.hx` cycles `Off -> Simple -> Advanced -> Off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DebugDisplayMode {
    #[default]
    Off,
    Simple,
    Advanced,
}

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

/// Pre-allocated fixed-capacity ring buffer for `FunkinStatsGraph` (100 samples, zero allocs).
#[derive(Debug, Clone)]
struct StatsGraph {
    history: [f32; GRAPH_HISTORY_MAX],
    start: usize,
    len: usize,
    min_value: f32,
    max_value: f32,
}

impl StatsGraph {
    const fn new(min_value: f32) -> Self {
        Self {
            history: [0.0; GRAPH_HISTORY_MAX],
            start: 0,
            len: 0,
            min_value,
            max_value: f32::NEG_INFINITY,
        }
    }

    fn push(&mut self, value: f32) {
        if self.len < GRAPH_HISTORY_MAX {
            let idx = (self.start + self.len) % GRAPH_HISTORY_MAX;
            self.history[idx] = value;
            self.len += 1;
        } else {
            self.history[self.start] = value;
            self.start = (self.start + 1) % GRAPH_HISTORY_MAX;
        }
        if value > self.max_value {
            self.max_value = value;
        }
        if value < self.min_value {
            self.min_value = value;
        }
    }

    fn average(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let mut sum = 0.0_f32;
        for i in 0..self.len {
            sum += self.history[(self.start + i) % GRAPH_HISTORY_MAX];
        }
        sum / (self.len as f32)
    }

    fn lowest(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let mut best = self.history[self.start];
        for i in 1..self.len {
            let v = self.history[(self.start + i) % GRAPH_HISTORY_MAX];
            if v < best {
                best = v;
            }
        }
        best
    }

    fn draw(&self, batcher: &mut SpriteBatcher, white_tex: TextureHandle, gx: f32, gy: f32, width: f32, height: f32) {
        let axis_x = gx + 4.0;
        let axis_color = [1.0, 1.0, 1.0, 0.5];
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([axis_x, gy], [1.0, height]).with_color(axis_color),
        );
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([axis_x, gy + height], [width, 1.0]).with_color(axis_color),
        );

        if self.len == 0 {
            return;
        }
        let inc = (width - 2.0) / ((GRAPH_HISTORY_MAX - 1) as f32);
        let range = (self.max_value - self.min_value).max(self.max_value * 0.1).max(1.0);
        let scale = height / range;

        let mut prev_x = axis_x;
        let v0 = self.history[self.start];
        let mut prev_y = gy + height - ((v0 - self.min_value) * scale) - 1.0;

        for i in 0..self.len {
            let v = self.history[(self.start + i) % GRAPH_HISTORY_MAX];
            let px = axis_x + 1.0 + (i as f32) * inc;
            let py = gy + height - ((v - self.min_value) * scale) - 1.0;
            let dx = px - prev_x;
            let dy = py - prev_y;
            let seg_len = dx.hypot(dy).max(1.0);
            let mut seg = SpriteInstance::new([prev_x, prev_y - 0.5], [seg_len, 1.2])
                .with_color([1.0, 1.0, 1.0, 1.0]);
            seg.rotation = dy.atan2(dx);
            seg.origin = [0.0, 0.5];
            batcher.push(white_tex, BlendMode::Normal, FilterMode::Linear, seg);
            prev_x = px;
            prev_y = py;
        }
    }
}

/// Zero-allocation `F6`-cycled (`Off -> Simple -> Advanced`) FPS & memory HUD (`FunkinDebugDisplay.hx`).
#[derive(Debug)]
pub struct FunkinDebugDisplay {
    pub mode: DebugDisplayMode,
    total_time_s: f64,
    frame_times_s: [f64; 512],
    frame_head: usize,
    frame_count: usize,
    delta_accum_s: f32,
    fps: u32,
    task_mem_bytes: Option<f64>,
    task_mem_peak_bytes: f64,
    fps_graph: StatsGraph,
    task_mem_graph: StatsGraph,
    fps_buf: StackTextBuffer<96>,
    mem_buf: StackTextBuffer<96>,
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
            mode: if visible {
                DebugDisplayMode::Simple
            } else {
                DebugDisplayMode::Off
            },
            total_time_s: 0.0,
            frame_times_s: [0.0; 512],
            frame_head: 0,
            frame_count: 0,
            delta_accum_s: 0.0,
            fps: 60,
            task_mem_bytes: win_mem::task_memory_bytes(),
            task_mem_peak_bytes: 0.0,
            fps_graph: StatsGraph::new(0.0),
            task_mem_graph: StatsGraph::new(0.0),
            fps_buf: StackTextBuffer::new(),
            mem_buf: StackTextBuffer::new(),
        }
    }

    /// Cycles `Off -> Simple -> Advanced -> Off` matching V-Slice `F6` (`Main.hx`).
    /// Returns whether the display remains active.
    pub fn cycle(&mut self) -> bool {
        self.mode = match self.mode {
            DebugDisplayMode::Off => DebugDisplayMode::Simple,
            DebugDisplayMode::Simple => DebugDisplayMode::Advanced,
            DebugDisplayMode::Advanced => DebugDisplayMode::Off,
        };
        self.mode != DebugDisplayMode::Off
    }

    /// Enables or disables the overlay directly (used by Options toggle).
    pub fn set_visible(&mut self, visible: bool) {
        self.mode = if visible {
            if self.mode == DebugDisplayMode::Advanced {
                DebugDisplayMode::Advanced
            } else {
                DebugDisplayMode::Simple
            }
        } else {
            DebugDisplayMode::Off
        };
    }

    #[inline]
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.mode != DebugDisplayMode::Off
    }

    /// Accumulates frame timestamps over a 1s sliding window and updates graphs every `0.1s`.
    pub fn update(&mut self, elapsed_s: f32) {
        let dt = elapsed_s.max(0.0);
        self.total_time_s += f64::from(dt);

        if self.frame_count < self.frame_times_s.len() {
            let tail = (self.frame_head + self.frame_count) % self.frame_times_s.len();
            self.frame_times_s[tail] = self.total_time_s;
            self.frame_count += 1;
        } else {
            self.frame_times_s[self.frame_head] = self.total_time_s;
            self.frame_head = (self.frame_head + 1) % self.frame_times_s.len();
        }

        let cutoff = self.total_time_s - 1.0;
        while self.frame_count > 0 && self.frame_times_s[self.frame_head] < cutoff {
            self.frame_head = (self.frame_head + 1) % self.frame_times_s.len();
            self.frame_count -= 1;
        }

        self.delta_accum_s += dt;
        if self.delta_accum_s < UPDATE_DELAY_S {
            return;
        }
        self.delta_accum_s = 0.0;

        self.fps = self.frame_count as u32;
        self.fps_graph.push(self.fps as f32);

        if let Some(mem) = win_mem::task_memory_bytes() {
            self.task_mem_bytes = Some(mem);
            if mem > self.task_mem_peak_bytes {
                self.task_mem_peak_bytes = mem;
            }
            self.task_mem_graph.push(mem as f32);
        }
    }

    /// Renders `FunkinDebugDisplay` at `(10, 10)` in `Simple` or `Advanced` mode. Zero allocations.
    pub fn draw(
        &mut self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
        _snap: &DebugSnapshot<'_>,
    ) {
        if self.mode == DebugDisplayMode::Off {
            return;
        }

        let advanced = self.mode == DebugDisplayMode::Advanced;
        let has_task_mem = self.task_mem_bytes.is_some();
        let bg_w_mult = if advanced || has_task_mem { 1.0 } else { 0.3 };
        let bg_h_mult = if has_task_mem {
            if advanced { 0.7 } else { 0.2 }
        } else if advanced {
            0.45
        } else {
            0.15
        };

        let outer_w = OUTER_RECT_DIMENSIONS[0] * bg_w_mult + INNER_RECT_DIFF * 2.0;
        let outer_h = OUTER_RECT_DIMENSIONS[1] * bg_h_mult + INNER_RECT_DIFF * 2.0;
        let inner_w = OUTER_RECT_DIMENSIONS[0] * bg_w_mult;
        let inner_h = OUTER_RECT_DIMENSIONS[1] * bg_h_mult;

        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new(PANEL_POS, [outer_w, outer_h]).with_color(BORDER_COLOR),
        );
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new(
                [PANEL_POS[0] + INNER_RECT_DIFF, PANEL_POS[1] + INNER_RECT_DIFF],
                [inner_w, inner_h],
            )
            .with_color(INNER_COLOR),
        );

        let scale = if font.line_height() <= 10.0 { 1.35 } else { 0.72 };
        let line_step = if font.line_height() <= 10.0 { 14.0 } else { 15.0 };
        let text_style = TextStyle {
            scale,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Left,
            ..Default::default()
        };

        let tx = PANEL_POS[0] + OTHERS_OFFSET;
        let ty = PANEL_POS[1] + OTHERS_OFFSET;

        self.fps_buf.clear();
        self.mem_buf.clear();
        if let Some(cur) = self.task_mem_bytes {
            let mut b1 = StackTextBuffer::<24>::new();
            let mut b2 = StackTextBuffer::<24>::new();
            format_bytes_lower(&mut b1, cur);
            format_bytes_lower(&mut b2, self.task_mem_peak_bytes);
            let _ = write!(self.mem_buf, "TASK MEM: {} / {}", b1.as_str(), b2.as_str());
        }

        if !advanced {
            let _ = write!(self.fps_buf, "FPS: {}", self.fps);
            font.draw_bytes(batcher, font_tex, self.fps_buf.as_bytes(), [tx, ty], &text_style);
            if has_task_mem {
                font.draw_bytes(
                    batcher,
                    font_tex,
                    self.mem_buf.as_bytes(),
                    [tx, ty + line_step],
                    &text_style,
                );
            }
            return;
        }

        let avg = self.fps_graph.average().floor() as u32;
        let low = self.fps_graph.lowest().floor() as u32;
        let _ = write!(self.fps_buf, "FPS: {}", self.fps);
        font.draw_bytes(batcher, font_tex, self.fps_buf.as_bytes(), [tx, ty], &text_style);

        self.fps_buf.clear();
        let _ = write!(self.fps_buf, "AVG FPS: {avg}");
        font.draw_bytes(
            batcher,
            font_tex,
            self.fps_buf.as_bytes(),
            [tx, ty + line_step],
            &text_style,
        );

        self.fps_buf.clear();
        let _ = write!(self.fps_buf, "1% LOW FPS: {low}");
        font.draw_bytes(
            batcher,
            font_tex,
            self.fps_buf.as_bytes(),
            [tx, ty + line_step * 2.0],
            &text_style,
        );

        let graphs_w = OUTER_RECT_DIMENSIONS[0] + INNER_RECT_DIFF * 2.0 - OTHERS_OFFSET * 3.0;
        let fps_gx = PANEL_POS[0] + OTHERS_OFFSET;
        let fps_gy = PANEL_POS[1] + OTHERS_OFFSET + 49.0;
        self.fps_graph
            .draw(batcher, white_tex, fps_gx, fps_gy, graphs_w, GRAPH_HEIGHT);

        if has_task_mem {
            let task_gy = PANEL_POS[1] + (OTHERS_OFFSET + (OTHERS_OFFSET + 49.0 + GRAPH_HEIGHT) + 22.0);
            font.draw_bytes(
                batcher,
                font_tex,
                self.mem_buf.as_bytes(),
                [task_gy * 0.0 + fps_gx, task_gy - 22.0],
                &text_style,
            );
            self.task_mem_graph
                .draw(batcher, white_tex, fps_gx, task_gy, graphs_w, GRAPH_HEIGHT);
        }
    }
}

fn format_bytes_lower(buf: &mut StackTextBuffer<24>, bytes: f64) {
    const UNITS: [&str; 4] = ["b", "kb", "mb", "gb"];
    let mut val = bytes.max(0.0);
    let mut idx = 0usize;
    while val >= 1024.0 && idx + 1 < UNITS.len() {
        val /= 1024.0;
        idx += 1;
    }
    let rounded = (val * 100.0).round() / 100.0;
    let _ = write!(buf, "{rounded}{}", UNITS[idx]);
}
