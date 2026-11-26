//! Zero-allocation V-Slice Sparrow atlas text (`AtlasText.hx`: `fonts/bold`, `fonts/default`).

use crate::atlas::{AtlasFrame, SparrowAtlas};
use crate::batch::{
    BlendMode, FilterMode, SpriteBatcher, SpriteInstance, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y,
    SPRITE_FLAG_ROTATED_90_CCW,
};
use crate::gpu::TextureHandle;
use crate::text::TextAlign;

const MAX_GLYPH_FRAMES: usize = 4;

#[derive(Debug, Clone)]
struct AtlasGlyph {
    sheet_idx: u8,
    frames: [Option<AtlasFrame>; MAX_GLYPH_FRAMES],
    frame_count: usize,
}

impl Default for AtlasGlyph {
    fn default() -> Self {
        Self {
            sheet_idx: 0,
            frames: [None, None, None, None],
            frame_count: 0,
        }
    }
}

/// Draw options for [`AtlasFont::draw_bytes`] / [`AtlasFont::draw_str`].
#[derive(Debug, Clone, Copy)]
pub struct AtlasTextStyle {
    pub scale: f32,
    pub color: [f32; 4],
    pub align: TextAlign,
    pub time_s: f32,
    pub animated: bool,
    pub line_height: f32,
}

impl Default for AtlasTextStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Left,
            time_s: 0.0,
            animated: true,
            line_height: 70.0,
        }
    }
}

/// Pre-indexed Sparrow atlas font (`bold.png` + `bold.xml`, with optional `default.png` fallback
/// for lowercase & digits).
#[derive(Debug, Clone)]
pub struct AtlasFont {
    textures: [Option<(TextureHandle, [f32; 2])>; 2],
    ascii: Vec<AtlasGlyph>, // len = 128
    space_advance: f32,
}

impl AtlasFont {
    /// Creates an empty font (no-op draw if assets absent).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            textures: [None, None],
            ascii: vec![AtlasGlyph::default(); 128],
            space_advance: 40.0,
        }
    }

    /// Builds a font from `bold` (`sheet 0`) and optional `default` (`sheet 1`) Sparrow atlases.
    #[must_use]
    pub fn from_sparrows(
        bold: Option<(TextureHandle, [f32; 2], &SparrowAtlas)>,
        default_sheet: Option<(TextureHandle, [f32; 2], &SparrowAtlas)>,
        prefer_bold: bool,
    ) -> Self {
        let mut font = Self::empty();
        if let Some((tex, sz, _)) = bold {
            font.textures[0] = Some((tex, sz));
        }
        if let Some((tex, sz, _)) = default_sheet {
            font.textures[1] = Some((tex, sz));
        }

        let bold_atlas = bold.map(|(_, _, a)| a);
        let def_atlas = default_sheet.map(|(_, _, a)| a);

        for code in 33u8..=126u8 {
            let ch = code as char;
            let glyph = if prefer_bold {
                lookup_char_glyph(bold_atlas, 0, ch, true)
                    .or_else(|| lookup_char_glyph(def_atlas, 1, ch, false))
            } else {
                lookup_char_glyph(def_atlas, 1, ch, false)
                    .or_else(|| lookup_char_glyph(bold_atlas, 0, ch, true))
            };
            if let Some(g) = glyph {
                font.ascii[code as usize] = g;
            }
        }
        font
    }

    /// Returns `true` if at least one sheet texture is loaded.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.textures[0].is_some() || self.textures[1].is_some()
    }

    /// Measures single-line width in virtual pixels without allocating.
    #[must_use]
    pub fn measure_line(&self, line: &[u8], scale: f32) -> f32 {
        let mut w = 0.0f32;
        for &b in line {
            if b == b' ' {
                w += self.space_advance * scale;
                continue;
            }
            let glyph = self.ascii.get(b as usize).filter(|g| g.frame_count > 0);
            if let Some(g) = glyph.and_then(|g| g.frames[0].as_ref()) {
                let src = g.source_size();
                w += (src[0] + 2.0) * scale;
            } else if b.is_ascii_graphic() {
                w += self.space_advance * scale;
            }
        }
        w
    }

    /// Draws UTF-8 ASCII bytes using pre-indexed Sparrow glyph frames with zero heap allocations.
    pub fn draw_bytes(
        &self,
        batcher: &mut SpriteBatcher,
        bytes: &[u8],
        pos: [f32; 2],
        style: &AtlasTextStyle,
    ) {
        if !self.is_loaded() {
            return;
        }
        let scale = style.scale.max(0.05);
        let step_y = style.line_height * scale;
        let frame_idx = if style.animated {
            (style.time_s.max(0.0) * 24.0).floor() as usize
        } else {
            0
        };

        let mut cursor_y = pos[1];
        for line in bytes.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let line_w = self.measure_line(line, scale);
            let mut cursor_x = match style.align {
                TextAlign::Left => pos[0],
                TextAlign::Center => pos[0] - line_w * 0.5,
                TextAlign::Right => pos[0] - line_w,
            };

            for &b in line {
                if b == b' ' {
                    cursor_x += self.space_advance * scale;
                    continue;
                }
                let Some(glyph) = self.ascii.get(b as usize).filter(|g| g.frame_count > 0) else {
                    if b.is_ascii_graphic() {
                        cursor_x += self.space_advance * scale;
                    }
                    continue;
                };
                let Some((tex, tex_sz)) = self.textures[glyph.sheet_idx as usize] else {
                    continue;
                };
                let Some(frame) = &glyph.frames[frame_idx % glyph.frame_count] else {
                    continue;
                };
                let src = frame.source_size();
                let trimmed = frame.upright_trimmed_size();
                let draw_pos = [
                    cursor_x - frame.frame_x * scale,
                    cursor_y + (65.0 - src[1]) * scale - frame.frame_y * scale,
                ];
                let draw_size = [trimmed[0] * scale, trimmed[1] * scale];
                let mut flags = 0u32;
                if frame.rotated {
                    flags |= SPRITE_FLAG_ROTATED_90_CCW;
                }
                if frame.flip_x {
                    flags |= SPRITE_FLAG_FLIP_X;
                }
                if frame.flip_y {
                    flags |= SPRITE_FLAG_FLIP_Y;
                }
                let inst = SpriteInstance::new(draw_pos, draw_size)
                    .with_uv_rect(frame.uv_bounds_inset(tex_sz[0], tex_sz[1], 0.5))
                    .with_color(style.color)
                    .with_flags(flags);
                batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
                cursor_x += (src[0] + 2.0) * scale;
            }

            cursor_y += step_y;
        }
    }

    /// Draws `&str` with zero heap allocations.
    #[inline]
    pub fn draw_str(&self, batcher: &mut SpriteBatcher, text: &str, pos: [f32; 2], style: &AtlasTextStyle) {
        self.draw_bytes(batcher, text.as_bytes(), pos, style);
    }
}

fn lookup_char_glyph(
    atlas: Option<&SparrowAtlas>,
    sheet_idx: u8,
    ch: char,
    fold_upper: bool,
) -> Option<AtlasGlyph> {
    let atlas = atlas?;
    let c = if fold_upper && ch.is_ascii_lowercase() {
        ch.to_ascii_uppercase()
    } else {
        ch
    };
    let mut tmp = [0u8; 4];
    let prefix: &str = match c {
        '&' => {
            if atlas.frames_by_prefix("-andpersand-").next().is_some() {
                "-andpersand-"
            } else {
                "&"
            }
        }
        '\'' => "-apostraphie-",
        '\\' => "-back slash-",
        ',' => "-comma-",
        '-' => "-dash-",
        '!' => "-exclamation point-",
        '/' => "-forward slash-",
        '>' => "-greater than-",
        '<' => "-less than-",
        '.' => "-period-",
        '?' => "-question mark-",
        '"' => "-start quote-",
        _ => c.encode_utf8(&mut tmp),
    };

    let mut frames: [Option<AtlasFrame>; MAX_GLYPH_FRAMES] = [None, None, None, None];
    let mut frame_count = 0usize;
    for f in atlas.frames_by_prefix(prefix).take(MAX_GLYPH_FRAMES) {
        frames[frame_count] = Some(f.clone());
        frame_count += 1;
    }
    if frame_count == 0 {
        return None;
    }
    Some(AtlasGlyph {
        sheet_idx,
        frames,
        frame_count,
    })
}
