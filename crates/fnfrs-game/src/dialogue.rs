//! V-Slice Week 6 Dialogue & Conversation system (`Conversation.hx` / `DialogueBox.hx`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::dialogue::{
    ConversationData, DialogueBoxData, DialogueLineData, DialogueSpeakerData,
};
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, StackTextBuffer,
    TextAlign, TextStyle, TextureHandle,
};

use crate::character::{
    build_loaded_anim, draw_atlas_frame, load_sparrow_source, AnimBank, AtlasDrawParams,
};
use crate::stage::parse_stage_color;

#[derive(Debug)]
struct LoadedDialogueBox {
    data: DialogueBoxData,
    bank: AnimBank,
    text_color: [f32; 4],
    shadow_color: [f32; 4],
}

#[derive(Debug)]
struct LoadedSpeaker {
    data: DialogueSpeakerData,
    bank: AnimBank,
}

/// Result emitted by [`DialogueState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogueAction {
    None,
    PlayTypeTick,
    PlayClick,
    Finished,
}

const CHARS_PER_SEC: f32 = 28.0;
const MAX_WRAPPED_LINES: usize = 5;

/// Runtime controller for V-Slice Week 6 dialogue sequences (`senpai`, `roses`, `thorns`).
#[derive(Debug)]
pub struct DialogueState {
    white_tex: TextureHandle,
    backdrop_color: [f32; 4],
    backdrop_fade_s: f32,
    outro_fade_s: f32,
    pub music_asset: Option<String>,
    pub music_looped: bool,
    lines: Vec<DialogueLineData>,
    boxes: HashMap<String, LoadedDialogueBox>,
    speakers: HashMap<String, LoadedSpeaker>,

    current_line_idx: usize,
    full_text: String,
    revealed_chars: usize,
    char_accum_s: f32,
    elapsed_s: f32,
    finishing_outro: bool,
    outro_timer_s: f32,
    pub is_active: bool,

    wrapped_lines: [StackTextBuffer<128>; MAX_WRAPPED_LINES],
    wrapped_line_count: usize,
    draw_line_buf: StackTextBuffer<128>,
}

impl DialogueState {
    /// Loads `data/dialogue/conversations/<id>.json` and all referenced boxes & speakers.
    pub fn load_by_id(
        conversation_id: &str,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Option<Self> {
        let path = assets.data_json(&format!("dialogue/conversations/{conversation_id}"));
        let raw = fs::read_to_string(&path).ok()?;
        let conv: ConversationData = serde_json::from_str(&raw).ok()?;
        if conv.dialogue.is_empty() {
            return None;
        }

        let backdrop_color = conv
            .backdrop
            .as_ref()
            .map_or([0.847, 0.875, 0.702, 0.75], |b| parse_stage_color(&b.color));
        let backdrop_fade_s = conv.backdrop.as_ref().map_or(1.0, |b| b.fade_time.max(0.05));
        let outro_fade_s = conv.outro.as_ref().map_or(0.8, |o| o.fade_time.max(0.05));
        let (music_asset, music_looped) = match &conv.music {
            Some(m) if !m.asset.trim().is_empty() => (Some(m.asset.clone()), m.looped),
            _ => (None, true),
        };

        let mut boxes = HashMap::new();
        let mut speakers = HashMap::new();

        for line in &conv.dialogue {
            if !boxes.contains_key(&line.dialogue_box) {
                if let Some(b) = load_box(&line.dialogue_box, assets, gpu) {
                    boxes.insert(line.dialogue_box.clone(), b);
                }
            }
            if !speakers.contains_key(&line.speaker) {
                if let Some(sp) = load_speaker(&line.speaker, assets, gpu) {
                    speakers.insert(line.speaker.clone(), sp);
                }
            }
        }

        let mut state = Self {
            white_tex: gpu.white_texture(),
            backdrop_color,
            backdrop_fade_s,
            outro_fade_s,
            music_asset,
            music_looped,
            lines: conv.dialogue,
            boxes,
            speakers,
            current_line_idx: 0,
            full_text: String::new(),
            revealed_chars: 0,
            char_accum_s: 0.0,
            elapsed_s: 0.0,
            finishing_outro: false,
            outro_timer_s: 0.0,
            is_active: true,
            wrapped_lines: std::array::from_fn(|_| StackTextBuffer::new()),
            wrapped_line_count: 0,
            draw_line_buf: StackTextBuffer::new(),
        };
        state.start_line(0);
        Some(state)
    }

    fn start_line(&mut self, idx: usize) {
        self.current_line_idx = idx;
        self.revealed_chars = 0;
        self.char_accum_s = 0.0;
        self.full_text.clear();

        let Some(line) = self.lines.get(idx).cloned() else {
            self.finishing_outro = true;
            self.outro_timer_s = 0.0;
            return;
        };

        self.full_text = line.text.join(" ");
        let max_cols = self
            .boxes
            .get(&line.dialogue_box)
            .map_or(36, |b| ((b.data.text.width / 22.0).round() as usize).clamp(18, 52));
        self.rewrap_text(max_cols);

        if let Some(bx) = self.boxes.get_mut(&line.dialogue_box) {
            let anim_idx = bx
                .bank
                .anim_index
                .get(&line.box_animation)
                .copied()
                .or_else(|| bx.bank.anim_index.get("enter").copied())
                .or_else(|| bx.bank.anim_index.get("idle").copied());
            if let Some(a_idx) = anim_idx {
                bx.bank.play_index(a_idx, true);
            }
        }
        if let Some(sp) = self.speakers.get_mut(&line.speaker) {
            let anim_idx = sp
                .bank
                .anim_index
                .get(&line.speaker_animation)
                .copied()
                .or_else(|| sp.bank.anim_index.get("talk").copied());
            if let Some(a_idx) = anim_idx {
                sp.bank.play_index(a_idx, true);
            }
        }
    }

    fn rewrap_text(&mut self, max_cols: usize) {
        for buf in &mut self.wrapped_lines {
            buf.clear();
        }
        self.wrapped_line_count = 0;

        let mut cur_line = 0usize;
        for word in self.full_text.split_whitespace() {
            if cur_line >= MAX_WRAPPED_LINES {
                break;
            }
            let cur_len = self.wrapped_lines[cur_line].len();
            let add_len = if cur_len == 0 {
                word.len()
            } else {
                word.len() + 1
            };
            if cur_len > 0 && cur_len + add_len > max_cols {
                cur_line += 1;
                if cur_line >= MAX_WRAPPED_LINES {
                    break;
                }
                let _ = self.wrapped_lines[cur_line].write_str(word);
            } else {
                if cur_len > 0 {
                    let _ = self.wrapped_lines[cur_line].write_str(" ");
                }
                let _ = self.wrapped_lines[cur_line].write_str(word);
            }
        }
        self.wrapped_line_count = (cur_line + 1).min(MAX_WRAPPED_LINES);
        if self.wrapped_lines[0].is_empty() {
            self.wrapped_line_count = 0;
        }
    }

    /// Advances typewriter reveal, sprites, and confirm/skip input with zero heap allocations.
    pub fn update(
        &mut self,
        elapsed_s: f32,
        accept_pressed: bool,
        skip_pressed: bool,
    ) -> DialogueAction {
        if !self.is_active {
            return DialogueAction::None;
        }

        let dt = elapsed_s.clamp(0.0, 0.1);
        self.elapsed_s += dt;

        if let Some(line) = self.lines.get(self.current_line_idx) {
            if let Some(bx) = self.boxes.get_mut(&line.dialogue_box) {
                let finished = bx.bank.update(dt);
                if finished && bx.bank.current_name() == "enter" {
                    if let Some(&idle_idx) = bx.bank.anim_index.get("idle") {
                        bx.bank.play_index(idle_idx, false);
                    }
                }
            }
            if let Some(sp) = self.speakers.get_mut(&line.speaker) {
                let finished = sp.bank.update(dt);
                if finished && sp.bank.current_name() == "talkEnter" {
                    if let Some(&talk_idx) = sp.bank.anim_index.get("talk") {
                        sp.bank.play_index(talk_idx, false);
                    }
                }
            }
        }

        if skip_pressed {
            self.is_active = false;
            return DialogueAction::Finished;
        }

        if self.finishing_outro {
            self.outro_timer_s += dt;
            if self.outro_timer_s >= self.outro_fade_s {
                self.is_active = false;
                return DialogueAction::Finished;
            }
            return DialogueAction::None;
        }

        let total_chars = self.full_text.chars().count();
        let mut out_action = DialogueAction::None;

        if self.revealed_chars < total_chars {
            self.char_accum_s += dt * CHARS_PER_SEC;
            let step = self.char_accum_s.floor() as usize;
            if step > 0 {
                self.char_accum_s -= step as f32;
                let prev = self.revealed_chars;
                self.revealed_chars = (self.revealed_chars + step).min(total_chars);
                if self.revealed_chars > prev {
                    out_action = DialogueAction::PlayTypeTick;
                }
            }
        }

        if accept_pressed {
            if self.revealed_chars < total_chars {
                self.revealed_chars = total_chars;
                return DialogueAction::PlayClick;
            }
            let next_idx = self.current_line_idx + 1;
            if next_idx < self.lines.len() {
                self.start_line(next_idx);
                return DialogueAction::PlayClick;
            }
            self.finishing_outro = true;
            self.outro_timer_s = 0.0;
            return DialogueAction::PlayClick;
        }

        out_action
    }

    /// Draws active backdrop, portrait, box sprite, and revealed typewriter text (zero heap allocations).
    pub fn draw(&mut self, batcher: &mut SpriteBatcher, font: &BitmapFont, font_tex: TextureHandle) {
        if !self.is_active {
            return;
        }

        let fade_in = (self.elapsed_s / self.backdrop_fade_s).clamp(0.0, 1.0);
        let fade_out = if self.finishing_outro {
            1.0 - (self.outro_timer_s / self.outro_fade_s).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let master_alpha = fade_in * fade_out;

        let mut bg = self.backdrop_color;
        bg[3] *= master_alpha;
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT]).with_color(bg),
        );

        let Some(line) = self.lines.get(self.current_line_idx) else {
            return;
        };

        if let Some(sp) = self.speakers.get(&line.speaker) {
            draw_dialogue_bank(
                batcher,
                &sp.bank,
                [
                    VIRTUAL_WIDTH * 0.5 + sp.data.offsets[0],
                    VIRTUAL_HEIGHT * 0.5 + sp.data.offsets[1],
                ],
                sp.data.scale,
                sp.data.flip_x,
                sp.data.is_pixel,
                master_alpha,
            );
        }

        let (text_offsets, text_color, shadow_color, shadow_w) =
            if let Some(bx) = self.boxes.get(&line.dialogue_box) {
                draw_dialogue_bank(
                    batcher,
                    &bx.bank,
                    [
                        VIRTUAL_WIDTH * 0.5 + bx.data.offsets[0],
                        VIRTUAL_HEIGHT * 0.5 + bx.data.offsets[1],
                    ],
                    bx.data.scale,
                    bx.data.flip_x,
                    bx.data.is_pixel,
                    master_alpha,
                );
                (
                    bx.data.text.offsets,
                    bx.text_color,
                    bx.shadow_color,
                    bx.data.text.shadow_width.max(1.0),
                )
            } else {
                (
                    [225.0, 450.0],
                    [0.247, 0.125, 0.129, 1.0],
                    [0.847, 0.580, 0.580, 1.0],
                    2.0,
                )
            };

        let text_scale = if font.line_height() <= 10.0 { 3.0 } else { 1.65 };
        let line_step = if font.line_height() <= 10.0 { 34.0 } else { 32.0 };
        let mut tc = text_color;
        tc[3] *= master_alpha;
        let mut sc = shadow_color;
        sc[3] *= master_alpha;

        let mut remaining = self.revealed_chars;
        for idx in 0..self.wrapped_line_count {
            if remaining == 0 {
                break;
            }
            let src_str = self.wrapped_lines[idx].as_str();
            let line_chars = src_str.chars().count();
            let take_chars = remaining.min(line_chars);
            remaining = remaining.saturating_sub(line_chars + 1);

            self.draw_line_buf.clear();
            for (c_idx, ch) in src_str.chars().enumerate() {
                if c_idx >= take_chars {
                    break;
                }
                let mut utf8 = [0u8; 4];
                let _ = self.draw_line_buf.write_str(ch.encode_utf8(&mut utf8));
            }
            let x = text_offsets[0];
            let y = text_offsets[1] + (idx as f32) * line_step;
            if sc[3] > 0.01 {
                font.draw_bytes(
                    batcher,
                    font_tex,
                    self.draw_line_buf.as_bytes(),
                    x + shadow_w,
                    y + shadow_w,
                    &TextStyle {
                        scale: text_scale,
                        color: sc,
                        ..Default::default()
                    },
                );
            }
            font.draw_bytes(
                batcher,
                font_tex,
                self.draw_line_buf.as_bytes(),
                x,
                y,
                &TextStyle {
                    scale: text_scale,
                    color: tc,
                    ..Default::default()
                },
            );
        }

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            b"SPACE / ENTER: NEXT   ESC: SKIP",
            [VIRTUAL_WIDTH - 28.0, VIRTUAL_HEIGHT - 32.0],
            &TextStyle {
                scale: if font.line_height() <= 10.0 { 1.5 } else { 0.85 },
                color: [1.0, 1.0, 1.0, 0.72 * master_alpha],
                align: TextAlign::Right,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 0.85 * master_alpha],
        );
    }
}

fn draw_dialogue_bank(
    batcher: &mut SpriteBatcher,
    bank: &AnimBank,
    pos: [f32; 2],
    scale: f32,
    flip_x: bool,
    is_pixel: bool,
    alpha: f32,
) {
    let Some(idx) = bank.current_anim else {
        return;
    };
    let Some(anim) = bank.anims.get(idx) else {
        return;
    };
    let (texture, tex_size, frames) = match (&anim.frames, bank.atlases.first()) {
        (
            crate::character::AnimFrames::Sheet { frames, .. },
            Some(crate::character::LoadedAtlasSource::Sparrow {
                texture, tex_size, ..
            }),
        ) => (*texture, *tex_size, frames),
        _ => return,
    };
    if frames.is_empty() {
        return;
    }
    let frame = &frames[bank.current_frame.min(frames.len() - 1)];
    let params = AtlasDrawParams {
        pos,
        scale: [scale, scale],
        anim_offsets: anim.offsets,
        global_offsets: [0.0, 0.0],
        scroll_factor: [0.0, 0.0],
        cam_pos: [0.0, 0.0],
        cam_zoom: 1.0,
        angle_deg: 0.0,
        flip_x,
        flip_y: false,
        color: [1.0, 1.0, 1.0, 1.0],
        alpha,
        blend: BlendMode::Normal,
        is_pixel,
        adjust_color: None,
        wiggle: None,
        ref_unscaled_size: None,
    };
    draw_atlas_frame(
        batcher,
        texture,
        tex_size,
        frame,
        &params,
        anim.flip_x,
        anim.flip_y,
    );
}

fn load_box(
    box_id: &str,
    assets: &AssetLocator,
    gpu: &mut GpuContext,
) -> Option<LoadedDialogueBox> {
    let path = assets.data_json(&format!("dialogue/boxes/{box_id}"));
    let raw = fs::read_to_string(&path).ok()?;
    let data: DialogueBoxData = serde_json::from_str(&raw).ok()?;
    let mut bank = AnimBank::new();
    if let Some(src) = load_sparrow_source(assets, gpu, &data.asset_path) {
        bank.atlases.push(src);
        for anim in &data.animations {
            if let Some(loaded) = build_loaded_anim(anim, &bank.atlases, 0) {
                bank.anims.push(loaded);
            }
        }
    }
    bank.finalize_links();
    let text_color = parse_stage_color(&data.text.color);
    let shadow_color = parse_stage_color(&data.text.shadow_color);
    Some(LoadedDialogueBox {
        data,
        bank,
        text_color,
        shadow_color,
    })
}

fn load_speaker(
    speaker_id: &str,
    assets: &AssetLocator,
    gpu: &mut GpuContext,
) -> Option<LoadedSpeaker> {
    let path = assets.data_json(&format!("dialogue/speakers/{speaker_id}"));
    let raw = fs::read_to_string(&path).ok()?;
    let data: DialogueSpeakerData = serde_json::from_str(&raw).ok()?;
    let mut bank = AnimBank::new();
    if let Some(src) = load_sparrow_source(assets, gpu, &data.asset_path) {
        bank.atlases.push(src);
        for anim in &data.animations {
            if let Some(loaded) = build_loaded_anim(anim, &bank.atlases, 0) {
                bank.anims.push(loaded);
            }
        }
    }
    bank.finalize_links();
    Some(LoadedSpeaker { data, bank })
}
