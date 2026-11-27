//! Shared helpers for scripted in-game & video cutscenes (`CutsceneAudioQueue`,
//! `SkipPromptState`, and `CutsceneAtlasProp`).

use std::fs;

use fnfrs_core::constants::VIRTUAL_WIDTH;
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    AtlasFrame, BlendMode, EvaluatedPart, FilterMode, GpuContext, SpriteBatcher, SpriteInstance,
    TextureHandle,
};

use crate::camera::FlxEase;
use crate::character::{
    draw_animate_parts, draw_atlas_frame, load_animate_source, load_sparrow_source,
    AnimFrames, AtlasDrawParams, LoadedAtlasSource,
};
use crate::character::atlas_bank::Affine2D;
use crate::video::{parse_srt, SubtitleCue};

/// External audio cue requested by an active cutscene step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CutsceneAudioRequest {
    #[default]
    None,
    PlayMusic { key: &'static str, looped: bool },
    PlaySound { key: &'static str },
    StopMusic,
    StopAllSfx,
    PauseBgLoop(bool),
}

/// Fixed-capacity queue of audio commands emitted per frame (zero heap allocations).
#[derive(Debug, Clone, Copy, Default)]
pub struct CutsceneAudioQueue {
    pub items: [CutsceneAudioRequest; 4],
    pub count: usize,
}

impl CutsceneAudioQueue {
    pub fn push(&mut self, req: CutsceneAudioRequest) {
        if req != CutsceneAudioRequest::None && self.count < self.items.len() {
            self.items[self.count] = req;
            self.count += 1;
        }
    }

    #[must_use]
    pub fn first_or_none(self) -> CutsceneAudioRequest {
        if self.count > 0 {
            self.items[0]
        } else {
            CutsceneAudioRequest::None
        }
    }
}

/// Official V-Slice in-game skip prompt (`Skip [ ENTER ]` at `(936, 618)`, 40px white w/ black outline).
#[derive(Debug, Default, Clone, Copy)]
pub struct SkipPromptState {
    pub visible: bool,
    pub showing: bool,
    pub can_skip: bool,
    pub skipped: bool,
    pub alpha: f32,
    reveal_t_s: f32,
    pub fade_out_s: f32,
}

impl SkipPromptState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: true,
            showing: false,
            can_skip: false,
            skipped: false,
            alpha: 0.0,
            reveal_t_s: 0.0,
            fade_out_s: 0.0,
        }
    }

    /// Updates skip prompt reveal/skip state; returns `Some(true)` once 0.5s post-skip fade completes.
    pub fn update(&mut self, dt: f32, advance_pressed: bool, allow_skip: bool) -> bool {
        if self.skipped {
            self.fade_out_s += dt;
            self.alpha = (1.0 - self.fade_out_s / 0.5).clamp(0.0, 1.0);
            return self.fade_out_s >= 0.5;
        }
        if !allow_skip {
            if self.showing {
                self.alpha = (self.alpha - dt * 2.0).clamp(0.0, 1.0);
                if self.alpha <= 0.0 {
                    self.visible = false;
                }
            }
            self.can_skip = false;
            return false;
        }

        if self.showing && !self.can_skip {
            self.reveal_t_s += dt;
            let t = (self.reveal_t_s / 0.5).clamp(0.0, 1.0);
            self.alpha = FlxEase::QuadOut.eval(t);
            if self.reveal_t_s >= 0.5 {
                self.can_skip = true;
            }
        }

        if advance_pressed {
            if self.can_skip {
                self.skipped = true;
                self.fade_out_s = 0.0;
            } else if !self.showing {
                self.showing = true;
                self.visible = true;
                self.reveal_t_s = 0.0;
            }
        }
        false
    }

    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: Option<TextureHandle>,
        font: &fnfrs_render::BitmapFont,
        font_tex: TextureHandle,
    ) {
        if self.visible && self.alpha > 0.01 {
            let scale = if font.line_height() <= 10.0 { 3.5 } else { 2.2 };
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                b"Skip [ ENTER ]",
                [936.0, 618.0],
                &fnfrs_render::TextStyle {
                    scale,
                    color: [1.0, 1.0, 1.0, self.alpha],
                    align: fnfrs_render::TextAlign::Left,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, self.alpha],
            );
        }

        if self.skipped {
            if let Some(wt) = white_tex {
                let fade = (self.fade_out_s / 0.5).clamp(0.0, 1.0);
                batcher.push(
                    wt,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 720.0])
                        .with_color([0.0, 0.0, 0.0, fade]),
                );
            }
        }
    }
}

/// Unified animated prop for scripted in-game cutscenes (AnimateAtlas timeline/label or Sparrow).
#[derive(Debug)]
pub struct CutsceneAtlasProp {
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    pub flip_x: bool,
    pub visible: bool,
    pub alpha: f32,
    pub looped: bool,
    pub fps: f32,
    pub adjust_color: Option<[f32; 4]>,

    source: LoadedAtlasSource,
    parts_buf: Vec<EvaluatedPart>,
    sparrow_frames: Vec<AtlasFrame>,
    stage_matrix: Option<Affine2D>,
    start_frame: u32,
    total_frames: u32,
    elapsed_s: f32,
    current_frame: u32,
    pub finished: bool,
}

impl CutsceneAtlasProp {
    /// Loads a whole-timeline or label-based Adobe Animate atlas prop.
    pub fn load_animate(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        key: &str,
        pos: [f32; 2],
        label: Option<&str>,
        looped: bool,
    ) -> Option<Self> {
        let source = load_animate_source(assets, gpu, key, None)?;
        let LoadedAtlasSource::Animate {
            atlas,
            apply_stage_matrix,
            ..
        } = &source
        else {
            return None;
        };
        let (start_frame, total_frames) = if let Some(lbl) = label {
            atlas
                .label_span(lbl)
                .map_or((0, atlas.total_frames().max(1)), |s| (s.start, s.duration.max(1)))
        } else {
            (0, atlas.total_frames().max(1))
        };
        let stage_matrix = if *apply_stage_matrix {
            let c = atlas.stage_matrix().to_cols_array();
            Some(Affine2D {
                a: c[0],
                b: c[1],
                c: c[2],
                d: c[3],
                tx: c[4],
                ty: c[5],
            })
        } else {
            None
        };
        let fps = atlas.frame_rate().max(1.0);
        let mut prop = Self {
            pos,
            scale: [1.0, 1.0],
            flip_x: false,
            visible: true,
            alpha: 1.0,
            looped,
            fps,
            adjust_color: None,
            source,
            parts_buf: Vec::with_capacity(64),
            sparrow_frames: Vec::new(),
            stage_matrix,
            start_frame,
            total_frames,
            elapsed_s: 0.0,
            current_frame: 0,
            finished: false,
        };
        prop.evaluate_current();
        Some(prop)
    }

    /// Loads a Sparrow XML prop with a prefix.
    pub fn load_sparrow(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        key: &str,
        prefix: &str,
        pos: [f32; 2],
        looped: bool,
    ) -> Option<Self> {
        let source = load_sparrow_source(assets, gpu, key)?;
        let frames: Vec<AtlasFrame> = match &source {
            LoadedAtlasSource::Sparrow { atlas, .. } => {
                atlas.frames_by_prefix(prefix).cloned().collect()
            }
            _ => return None,
        };
        if frames.is_empty() {
            return None;
        }
        let total = frames.len() as u32;
        Some(Self {
            pos,
            scale: [1.0, 1.0],
            flip_x: false,
            visible: true,
            alpha: 1.0,
            looped,
            fps: 24.0,
            adjust_color: None,
            source,
            parts_buf: Vec::new(),
            sparrow_frames: frames,
            stage_matrix: None,
            start_frame: 0,
            total_frames: total,
            elapsed_s: 0.0,
            current_frame: 0,
            finished: false,
        })
    }

    /// Switches an Animate atlas prop to another frame label cleanly.
    pub fn play_label(&mut self, label: &str, looped: bool) {
        if let LoadedAtlasSource::Animate { atlas, .. } = &self.source {
            if let Some(span) = atlas.label_span(label) {
                self.start_frame = span.start;
                self.total_frames = span.duration.max(1);
                self.elapsed_s = 0.0;
                self.current_frame = 0;
                self.looped = looped;
                self.finished = false;
                self.evaluate_current();
            }
        }
    }

    /// Jumps directly to `elapsed_s` inside the active animation (e.g. skip-to-frame 23).
    pub fn seek_frame(&mut self, frame: u32) {
        self.current_frame = frame.min(self.total_frames.saturating_sub(1));
        self.elapsed_s = self.current_frame as f32 / self.fps.max(1.0);
        self.evaluate_current();
    }

    pub fn update(&mut self, dt: f32) {
        if !self.visible || self.total_frames == 0 {
            return;
        }
        self.elapsed_s += dt.max(0.0);
        let raw = (self.elapsed_s * self.fps).floor() as u32;
        let next = if self.looped {
            raw % self.total_frames
        } else {
            if raw >= self.total_frames.saturating_sub(1) {
                self.finished = true;
            }
            raw.min(self.total_frames.saturating_sub(1))
        };
        if next != self.current_frame {
            self.current_frame = next;
            self.evaluate_current();
        }
    }

    fn evaluate_current(&mut self) {
        if let LoadedAtlasSource::Animate { atlas, .. } = &self.source {
            atlas.evaluate_frame(self.start_frame + self.current_frame, &mut self.parts_buf);
        }
    }

    pub fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.visible || self.alpha <= 0.001 {
            return;
        }
        let params = AtlasDrawParams {
            pos: self.pos,
            scale: self.scale,
            anim_offsets: [0.0, 0.0],
            global_offsets: [0.0, 0.0],
            scroll_factor: [1.0, 1.0],
            cam_pos,
            cam_zoom,
            angle_deg: 0.0,
            flip_x: self.flip_x,
            flip_y: false,
            color: [1.0, 1.0, 1.0, 1.0],
            alpha: self.alpha,
            blend: BlendMode::Normal,
            is_pixel: false,
            adjust_color: self.adjust_color,
            wiggle: None,
            ref_unscaled_size: None,
        };
        match &self.source {
            LoadedAtlasSource::Animate {
                textures,
                bounds_origin,
                bounds_size,
                ..
            } => {
                draw_animate_parts(
                    batcher,
                    textures,
                    &self.parts_buf,
                    *bounds_origin,
                    *bounds_size,
                    self.stage_matrix,
                    &params,
                    false,
                    false,
                );
            }
            LoadedAtlasSource::Sparrow {
                texture, tex_size, ..
            } => {
                if let Some(frame) = self.sparrow_frames.get(self.current_frame as usize) {
                    draw_atlas_frame(
                        batcher, *texture, *tex_size, frame, &params, false, false,
                    );
                }
            }
            LoadedAtlasSource::Packer { .. } => {}
        }
        let _ = AnimFrames::Sheet {
            atlas_idx: 0,
            frames: Vec::new(),
        };
    }
}

/// Loads `assets/data/<rel>.srt` subtitle cues if present.
pub fn load_subtitles(assets: &AssetLocator, rel_no_ext: &str) -> Vec<SubtitleCue> {
    let path = assets.resolve_key("data", rel_no_ext, Some("srt"));
    fs::read_to_string(path)
        .ok()
        .map(|raw| parse_srt(&raw))
        .unwrap_or_default()
}

/// Draws active `.srt` subtitle cue matching `elapsed_s` at bottom-center.
pub fn draw_subtitle_cue(
    cues: &[SubtitleCue],
    elapsed_s: f32,
    batcher: &mut SpriteBatcher,
    font: &fnfrs_render::BitmapFont,
    font_tex: TextureHandle,
    buf: &mut fnfrs_render::StackTextBuffer<160>,
    y_offset_px: f32,
) {
    let Some(cue) = cues
        .iter()
        .find(|c| elapsed_s >= c.start_s && elapsed_s <= c.end_s)
    else {
        return;
    };
    use std::fmt::Write as _;
    buf.clear();
    let _ = write!(buf, "{}", cue.text);
    font.draw_bytes_outlined(
        batcher,
        font_tex,
        buf.as_bytes(),
        [VIRTUAL_WIDTH * 0.5, 720.0 - 84.0 + y_offset_px],
        &fnfrs_render::TextStyle {
            scale: if font.line_height() <= 10.0 { 2.5 } else { 1.4 },
            color: [1.0, 1.0, 1.0, 1.0],
            align: fnfrs_render::TextAlign::Center,
            ..Default::default()
        },
        [0.0, 0.0, 0.0, 0.95],
    );
}
