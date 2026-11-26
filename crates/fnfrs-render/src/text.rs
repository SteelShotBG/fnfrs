//! Bitmap and pre-baked TTF/OTF font layout and rendering with zero
//! per-frame heap allocations.
//!
//! Supports four font sources:
//! 1. Embedded 5x7 ASCII (`32..=126`) bitmap fallback generated into RGBA8 once
//!    at initialization so HUD/Score text renders out-of-the-box without loose
//!    font assets.
//! 2. AngelCode BMFont `.fnt` in both XML (`vcr-bmp.fnt`) and plain-text
//!    formats.
//! 3. Sparrow V2 XML bitmap fonts (`assets/images/fonts/{bold,default}.xml`)
//!    following `funkin/ui/AtlasText.hx` symbol naming and case restriction.
//! 4. Pre-baked TTF/OTF glyph atlases rasterized once via `fontdue`.

use std::fmt::Write as _;

use rustc_hash::FxHashMap;
use thiserror::Error;

use crate::batch::{BlendMode, FilterMode, SpriteBatcher, SpriteInstance};
use crate::gpu::{GpuContext, GpuError, TextureHandle};

/// Default alpha cutoff threshold used to convert anti-aliased BMFont sheets
/// (such as `vcr-bmp.png`) into crisp binary-alpha pixel glyphs.
pub const DEFAULT_BMFONT_ALPHA_THRESHOLD: u8 = 112;

/// Errors produced when loading or parsing bitmap or TTF/OTF fonts.
#[derive(Debug, Error)]
pub enum FontError {
    #[error("invalid XML font document: {0}")]
    Xml(#[from] roxmltree::Error),

    #[error("missing required font data: {0}")]
    InvalidFormat(String),

    #[error("failed to parse TTF/OTF font: {0}")]
    TtfParse(String),

    #[error("failed to decode font PNG texture: {0}")]
    Image(#[from] image::ImageError),

    #[error("failed to upload font texture to GPU: {0}")]
    Gpu(#[from] GpuError),
}

/// Horizontal alignment for multi-line or single-line text layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Case handling mode for fonts that only provide uppercase or lowercase glyphs
/// (such as `bold.xml` in Friday Night Funkin').
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontCase {
    #[default]
    Both,
    Upper,
    Lower,
}

/// Parameters controlling zero-allocation text layout and drawing.
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub scale: f32,
    pub color: [f32; 4],
    pub align: TextAlign,
    pub letter_spacing: f32,
    pub line_spacing: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Left,
            letter_spacing: 0.0,
            line_spacing: 0.0,
        }
    }
}

/// Metrics and normalized UV rectangle for a single glyph in a font atlas.
#[derive(Debug, Clone, Copy, Default)]
pub struct Glyph {
    /// Normalized `[u0, v0, u1, v1]` in `[0.0, 1.0]` texture space.
    pub uv_rect: [f32; 4],
    /// Pixel width of the glyph quad.
    pub width: f32,
    /// Pixel height of the glyph quad.
    pub height: f32,
    /// Horizontal offset from cursor to glyph left edge.
    pub x_offset: f32,
    /// Vertical offset from line top to glyph top edge.
    pub y_offset: f32,
    /// Horizontal cursor advance after placing this glyph.
    pub x_advance: f32,
}

/// Positioned glyph quad emitted during zero-allocation layout.
#[derive(Debug, Clone, Copy)]
pub struct GlyphQuad {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub uv_rect: [f32; 4],
}

/// Pre-baked RGBA8 texture data generated once when building an embedded or TTF
/// font atlas.
#[derive(Debug, Clone)]
pub struct FontAtlasImage {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

/// Pre-indexed bitmap font supporting O(1) zero-allocation ASCII lookup and
/// `FxHashMap` fallback for non-ASCII glyphs.
#[derive(Debug, Clone)]
pub struct BitmapFont {
    ascii: [Option<Glyph>; 128],
    extra: FxHashMap<char, Glyph>,
    kernings: FxHashMap<(char, char), f32>,
    line_height: f32,
    base: f32,
    space_advance: f32,
    case_mode: FontCase,
    tex_width: u32,
    tex_height: u32,
}

impl BitmapFont {
    /// Generates the built-in 5x7 ASCII (`32..=126`) bitmap fallback font and
    /// its RGBA8 atlas once at initialization.
    ///
    /// Returns `(font, atlas_image)` so the caller can upload `atlas_image` to
    /// the GPU once and reuse `font` with zero per-frame allocations.
    #[must_use]
    pub fn embedded_fallback() -> (Self, FontAtlasImage) {
        const COLS: u32 = 16;
        const ROWS: u32 = 6;
        const GLYPH_W: u32 = 5;
        const GLYPH_H: u32 = 7;
        const CELL_W: u32 = 8;
        const CELL_H: u32 = 10;
        const ATLAS_W: u32 = COLS * CELL_W; // 128
        const ATLAS_H: u32 = ROWS * CELL_H; // 60

        let mut rgba8 = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let mut ascii: [Option<Glyph>; 128] = [None; 128];

        for (idx, rows) in EMBEDDED_5X7.iter().enumerate() {
            let code = 32usize + idx;
            let col = (idx as u32) % COLS;
            let row = (idx as u32) / COLS;
            let px_x = col * CELL_W + 1;
            let px_y = row * CELL_H + 1;

            for (ry, &bits) in rows.iter().enumerate() {
                for rx in 0..GLYPH_W {
                    if (bits & (1u8 << (GLYPH_W - 1 - rx))) != 0 {
                        let x = px_x + rx;
                        let y = px_y + ry as u32;
                        let offset = ((y * ATLAS_W + x) * 4) as usize;
                        rgba8[offset] = 255;
                        rgba8[offset + 1] = 255;
                        rgba8[offset + 2] = 255;
                        rgba8[offset + 3] = 255;
                    }
                }
            }

            let u0 = px_x as f32 / ATLAS_W as f32;
            let v0 = px_y as f32 / ATLAS_H as f32;
            let u1 = (px_x + GLYPH_W) as f32 / ATLAS_W as f32;
            let v1 = (px_y + GLYPH_H) as f32 / ATLAS_H as f32;

            ascii[code] = Some(Glyph {
                uv_rect: [u0, v0, u1, v1],
                width: if code == 32 { 0.0 } else { GLYPH_W as f32 },
                height: if code == 32 { 0.0 } else { GLYPH_H as f32 },
                x_offset: 0.0,
                y_offset: 0.0,
                x_advance: 6.0,
            });
        }

        let font = Self {
            ascii,
            extra: FxHashMap::default(),
            kernings: FxHashMap::default(),
            line_height: 9.0,
            base: 7.0,
            space_advance: 6.0,
            case_mode: FontCase::Both,
            tex_width: ATLAS_W,
            tex_height: ATLAS_H,
        };

        let image = FontAtlasImage {
            width: ATLAS_W,
            height: ATLAS_H,
            rgba8,
        };

        (font, image)
    }

    /// Creates the built-in 5x7 ASCII fallback font and immediately uploads its
    /// RGBA8 glyph atlas to `gpu` once at initialization.
    pub fn create_embedded_on_gpu(gpu: &mut GpuContext) -> Result<(Self, TextureHandle), GpuError> {
        let (font, image) = Self::embedded_fallback();
        let tex = gpu.create_texture_rgba8(
            Some("fnfrs-embedded-font-5x7"),
            image.width,
            image.height,
            &image.rgba8,
        )?;
        Ok((font, tex))
    }

    /// Loads an AngelCode BMFont (`.fnt` + `.png` bytes) and uploads its texture
    /// to `gpu`. When `crisp_alpha` is `true`, anti-aliased alpha fringes are
    /// snapped to binary `0` or `255` (`>= DEFAULT_BMFONT_ALPHA_THRESHOLD`) so
    /// bitmap fonts such as `vcr-bmp` render with a grounded pixel-art edge.
    pub fn create_bmfont_on_gpu(
        gpu: &mut GpuContext,
        label: Option<&str>,
        fnt_data: &str,
        png_bytes: &[u8],
        crisp_alpha: bool,
    ) -> Result<(Self, TextureHandle), FontError> {
        let rgba_img = image::load_from_memory(png_bytes)?.into_rgba8();
        let (width, height) = rgba_img.dimensions();
        let mut raw = rgba_img.into_raw();
        if crisp_alpha {
            crisp_alpha_rgba8(&mut raw, DEFAULT_BMFONT_ALPHA_THRESHOLD);
        }
        let tex = gpu.create_texture_rgba8(label, width, height, &raw)?;
        let font = Self::from_bmfont(fnt_data, Some(width), Some(height))?;
        Ok((font, tex))
    }

    /// Parses an AngelCode BMFont `.fnt` file in either XML (`vcr-bmp.fnt`) or
    /// plain-text format.
    ///
    /// If `atlas_width` / `atlas_height` are `Some`, they override `<common
    /// scaleW="..." scaleH="..."/>` when computing UVs.
    pub fn from_bmfont(
        data: &str,
        atlas_width: Option<u32>,
        atlas_height: Option<u32>,
    ) -> Result<Self, FontError> {
        let trimmed = data.trim_start_matches('\u{feff}').trim_start();
        if trimmed.starts_with('<') {
            Self::from_bmfont_xml(trimmed, atlas_width, atlas_height)
        } else {
            Self::from_bmfont_text(trimmed, atlas_width, atlas_height)
        }
    }

    /// Parses an AngelCode BMFont XML document (`vcr-bmp.fnt`).
    pub fn from_bmfont_xml(
        xml: &str,
        atlas_width: Option<u32>,
        atlas_height: Option<u32>,
    ) -> Result<Self, FontError> {
        let doc = roxmltree::Document::parse(xml)?;
        let root = doc.root_element();
        if root.tag_name().name() != "font" {
            return Err(FontError::InvalidFormat(
                "expected <font> root element in BMFont XML".to_owned(),
            ));
        }

        let mut line_height = 16.0f32;
        let mut base = 12.0f32;
        let mut scale_w = atlas_width.unwrap_or(0);
        let mut scale_h = atlas_height.unwrap_or(0);

        if let Some(common) = root.children().find(|n| n.has_tag_name("common")) {
            if let Some(v) = parse_attr_f32(&common, "lineHeight") {
                line_height = v;
            }
            if let Some(v) = parse_attr_f32(&common, "base") {
                base = v;
            }
            if scale_w == 0 {
                scale_w = parse_attr_u32(&common, "scaleW").unwrap_or(0);
            }
            if scale_h == 0 {
                scale_h = parse_attr_u32(&common, "scaleH").unwrap_or(0);
            }
        }

        if scale_w == 0 || scale_h == 0 {
            return Err(FontError::InvalidFormat(
                "BMFont XML missing non-zero scaleW/scaleH or texture dimensions".to_owned(),
            ));
        }

        let mut font = Self::empty(line_height, base, line_height * 0.5, scale_w, scale_h);

        if let Some(chars_node) = root.children().find(|n| n.has_tag_name("chars")) {
            for ch_node in chars_node.children().filter(|n| n.has_tag_name("char")) {
                let Some(id) = parse_attr_u32(&ch_node, "id") else {
                    continue;
                };
                let Some(ch) = char::from_u32(id) else {
                    continue;
                };
                let x = parse_attr_f32(&ch_node, "x").unwrap_or(0.0);
                let y = parse_attr_f32(&ch_node, "y").unwrap_or(0.0);
                let w = parse_attr_f32(&ch_node, "width").unwrap_or(0.0);
                let h = parse_attr_f32(&ch_node, "height").unwrap_or(0.0);
                let x_offset = parse_attr_f32(&ch_node, "xoffset").unwrap_or(0.0);
                let y_offset = parse_attr_f32(&ch_node, "yoffset").unwrap_or(0.0);
                let x_advance = parse_attr_f32(&ch_node, "xadvance").unwrap_or(w);

                let glyph = Glyph {
                    uv_rect: pixel_uv(x, y, w, h, scale_w, scale_h),
                    width: w,
                    height: h,
                    x_offset,
                    y_offset,
                    x_advance,
                };
                font.insert_glyph(ch, glyph);
                if ch == ' ' {
                    font.space_advance = x_advance;
                }
            }
        }

        if let Some(kernings_node) = root.children().find(|n| n.has_tag_name("kernings")) {
            for k_node in kernings_node
                .children()
                .filter(|n| n.has_tag_name("kerning"))
            {
                if let (Some(first), Some(second), Some(amount)) = (
                    parse_attr_u32(&k_node, "first").and_then(char::from_u32),
                    parse_attr_u32(&k_node, "second").and_then(char::from_u32),
                    parse_attr_f32(&k_node, "amount"),
                ) {
                    font.kernings.insert((first, second), amount);
                }
            }
        }

        Ok(font)
    }

    /// Parses a plain-text AngelCode BMFont `.fnt` document.
    pub fn from_bmfont_text(
        text: &str,
        atlas_width: Option<u32>,
        atlas_height: Option<u32>,
    ) -> Result<Self, FontError> {
        let mut line_height = 16.0f32;
        let mut base = 12.0f32;
        let mut scale_w = atlas_width.unwrap_or(0);
        let mut scale_h = atlas_height.unwrap_or(0);

        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("common ") {
                for (k, v) in BmfontKvIter::new(rest) {
                    match k {
                        "lineHeight" => {
                            if let Ok(parsed) = v.parse::<f32>() {
                                line_height = parsed;
                            }
                        }
                        "base" => {
                            if let Ok(parsed) = v.parse::<f32>() {
                                base = parsed;
                            }
                        }
                        "scaleW" if scale_w == 0 => {
                            if let Ok(parsed) = v.parse::<u32>() {
                                scale_w = parsed;
                            }
                        }
                        "scaleH" if scale_h == 0 => {
                            if let Ok(parsed) = v.parse::<u32>() {
                                scale_h = parsed;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if scale_w == 0 || scale_h == 0 {
            return Err(FontError::InvalidFormat(
                "BMFont text missing non-zero scaleW/scaleH or texture dimensions".to_owned(),
            ));
        }

        let mut font = Self::empty(line_height, base, line_height * 0.5, scale_w, scale_h);

        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("char ") {
                let mut id = None;
                let mut x = 0.0f32;
                let mut y = 0.0f32;
                let mut w = 0.0f32;
                let mut h = 0.0f32;
                let mut x_offset = 0.0f32;
                let mut y_offset = 0.0f32;
                let mut x_advance = None;

                for (k, v) in BmfontKvIter::new(rest) {
                    match k {
                        "id" => id = v.parse::<u32>().ok(),
                        "x" => x = v.parse::<f32>().unwrap_or(0.0),
                        "y" => y = v.parse::<f32>().unwrap_or(0.0),
                        "width" => w = v.parse::<f32>().unwrap_or(0.0),
                        "height" => h = v.parse::<f32>().unwrap_or(0.0),
                        "xoffset" => x_offset = v.parse::<f32>().unwrap_or(0.0),
                        "yoffset" => y_offset = v.parse::<f32>().unwrap_or(0.0),
                        "xadvance" => x_advance = v.parse::<f32>().ok(),
                        _ => {}
                    }
                }

                if let Some(ch) = id.and_then(char::from_u32) {
                    let adv = x_advance.unwrap_or(w);
                    let glyph = Glyph {
                        uv_rect: pixel_uv(x, y, w, h, scale_w, scale_h),
                        width: w,
                        height: h,
                        x_offset,
                        y_offset,
                        x_advance: adv,
                    };
                    font.insert_glyph(ch, glyph);
                    if ch == ' ' {
                        font.space_advance = adv;
                    }
                }
            } else if let Some(rest) = line.strip_prefix("kerning ") {
                let mut first = None;
                let mut second = None;
                let mut amount = None;
                for (k, v) in BmfontKvIter::new(rest) {
                    match k {
                        "first" => first = v.parse::<u32>().ok().and_then(char::from_u32),
                        "second" => second = v.parse::<u32>().ok().and_then(char::from_u32),
                        "amount" => amount = v.parse::<f32>().ok(),
                        _ => {}
                    }
                }
                if let (Some(f), Some(s), Some(a)) = (first, second, amount) {
                    font.kernings.insert((f, s), a);
                }
            }
        }

        Ok(font)
    }

    /// Parses a Sparrow V2 bitmap font XML (`bold.xml`, `default.xml`).
    ///
    /// Matches `funkin/ui/AtlasText.hx`: maps named symbol prefixes
    /// (`-andpersand-`, `-period-`, etc.), uses the first frame of each
    /// character, bottom-aligns glyphs to `max_height`, and infers
    /// `FontCase::Upper` / `FontCase::Lower` when only one letter case exists.
    pub fn from_sparrow_xml(
        xml: &str,
        tex_width: u32,
        tex_height: u32,
    ) -> Result<Self, FontError> {
        if tex_width == 0 || tex_height == 0 {
            return Err(FontError::InvalidFormat(
                "Sparrow font requires non-zero texture dimensions".to_owned(),
            ));
        }

        let doc = roxmltree::Document::parse(xml)?;
        let root = doc.root_element();
        if root.tag_name().name() != "TextureAtlas" {
            return Err(FontError::InvalidFormat(
                "expected <TextureAtlas> root element".to_owned(),
            ));
        }

        struct RawFrame {
            ch: char,
            x: f32,
            y: f32,
            w: f32,
            h: f32,
            frame_x: f32,
            frame_y: f32,
        }

        let mut seen: FxHashMap<char, RawFrame> = FxHashMap::default();
        let mut max_height = 0.0f32;
        let mut has_upper = false;
        let mut has_lower = false;

        for node in root.children().filter(|n| n.has_tag_name("SubTexture")) {
            let Some(name) = node.attribute("name") else {
                continue;
            };
            let prefix = strip_sparrow_frame_suffix(name);
            let Some(ch) = sparrow_prefix_to_char(prefix) else {
                continue;
            };

            let x = parse_attr_f32(&node, "x").unwrap_or(0.0);
            let y = parse_attr_f32(&node, "y").unwrap_or(0.0);
            let w = parse_attr_f32(&node, "width").unwrap_or(0.0);
            let h = parse_attr_f32(&node, "height").unwrap_or(0.0);
            let frame_x = parse_attr_f32(&node, "frameX").unwrap_or(0.0);
            let frame_y = parse_attr_f32(&node, "frameY").unwrap_or(0.0);

            if h > max_height {
                max_height = h;
            }
            if ch.is_ascii_uppercase() {
                has_upper = true;
            } else if ch.is_ascii_lowercase() {
                has_lower = true;
            }

            // Keep the first animation frame (typically `*0000`).
            seen.entry(ch).or_insert(RawFrame {
                ch,
                x,
                y,
                w,
                h,
                frame_x,
                frame_y,
            });
        }

        if max_height <= 0.0 {
            max_height = 48.0;
        }

        let mut font = Self::empty(max_height, max_height, 40.0, tex_width, tex_height);
        if has_upper != has_lower {
            font.case_mode = if has_upper {
                FontCase::Upper
            } else {
                FontCase::Lower
            };
        }

        for frame in seen.into_values() {
            let glyph = Glyph {
                uv_rect: pixel_uv(frame.x, frame.y, frame.w, frame.h, tex_width, tex_height),
                width: frame.w,
                height: frame.h,
                x_offset: -frame.frame_x,
                y_offset: (max_height - frame.h) - frame.frame_y,
                x_advance: frame.w,
            };
            font.insert_glyph(frame.ch, glyph);
        }

        // Alias ASCII `"` to typographic quote glyph if present in AtlasText fonts.
        if font.lookup_raw('"').is_none() {
            let quote = font.lookup_raw('“').or_else(|| font.lookup_raw('”'));
            if let Some(q) = quote {
                font.insert_glyph('"', q);
            }
        }

        Ok(font)
    }

    /// Pre-bakes a TTF or OTF font into an RGBA8 glyph atlas using `fontdue` once
    /// at load time.
    ///
    /// By default, rasterizes printable ASCII (`32..=126`) plus any extra
    /// characters listed in `extra_chars`.
    pub fn from_ttf_bytes(
        bytes: &[u8],
        px_size: f32,
        extra_chars: &[char],
    ) -> Result<(Self, FontAtlasImage), FontError> {
        if px_size <= 0.0 || !px_size.is_finite() {
            return Err(FontError::InvalidFormat(
                "TTF pixel size must be positive and finite".to_owned(),
            ));
        }

        let ttf = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .map_err(|err| FontError::TtfParse(err.to_owned()))?;

        let line_metrics = ttf.horizontal_line_metrics(px_size);
        let ascent = line_metrics.map_or(px_size * 0.8, |m| m.ascent);
        let line_height = line_metrics.map_or(px_size * 1.2, |m| m.new_line_size.max(px_size));

        let mut chars: Vec<char> = (32u8..=126u8).map(char::from).collect();
        for &ch in extra_chars {
            if !chars.contains(&ch) {
                chars.push(ch);
            }
        }

        struct Rasterized {
            ch: char,
            metrics: fontdue::Metrics,
            bitmap: Vec<u8>,
        }

        let mut rasterized = Vec::with_capacity(chars.len());
        let mut total_area: u32 = 0;
        let mut max_glyph_w: u32 = 1;

        for ch in chars {
            let (metrics, bitmap) = ttf.rasterize(ch, px_size);
            let gw = metrics.width as u32;
            let gh = metrics.height as u32;
            max_glyph_w = max_glyph_w.max(gw + 2);
            total_area += (gw + 2) * (gh + 2);
            rasterized.push(Rasterized {
                ch,
                metrics,
                bitmap,
            });
        }

        // Choose a power-of-two atlas width wide enough for shelf packing.
        let approx_side = ((total_area as f32).sqrt().ceil() as u32).max(max_glyph_w);
        let atlas_w = approx_side.next_power_of_two().clamp(128, 2048);

        let mut cursor_x = 1u32;
        let mut cursor_y = 1u32;
        let mut shelf_h = 0u32;

        struct Placed {
            ch: char,
            x: u32,
            y: u32,
            metrics: fontdue::Metrics,
            bitmap: Vec<u8>,
        }

        let mut placed = Vec::with_capacity(rasterized.len());
        for item in rasterized {
            let gw = item.metrics.width as u32;
            let gh = item.metrics.height as u32;
            if gw == 0 || gh == 0 {
                placed.push(Placed {
                    ch: item.ch,
                    x: 0,
                    y: 0,
                    metrics: item.metrics,
                    bitmap: item.bitmap,
                });
                continue;
            }

            if cursor_x + gw + 1 > atlas_w {
                cursor_x = 1;
                cursor_y += shelf_h + 1;
                shelf_h = 0;
            }

            placed.push(Placed {
                ch: item.ch,
                x: cursor_x,
                y: cursor_y,
                metrics: item.metrics,
                bitmap: item.bitmap,
            });

            cursor_x += gw + 1;
            shelf_h = shelf_h.max(gh);
        }

        let atlas_h = (cursor_y + shelf_h + 1).next_power_of_two().max(32);
        let mut rgba8 = vec![0u8; (atlas_w * atlas_h * 4) as usize];
        let mut font = Self::empty(line_height, ascent, px_size * 0.35, atlas_w, atlas_h);

        for item in placed {
            let gw = item.metrics.width as u32;
            let gh = item.metrics.height as u32;
            for ry in 0..gh {
                for rx in 0..gw {
                    let alpha = item.bitmap[(ry * gw + rx) as usize];
                    let dst = (((item.y + ry) * atlas_w + (item.x + rx)) * 4) as usize;
                    rgba8[dst] = 255;
                    rgba8[dst + 1] = 255;
                    rgba8[dst + 2] = 255;
                    rgba8[dst + 3] = alpha;
                }
            }

            let y_offset = ascent - (item.metrics.ymin as f32 + item.metrics.height as f32);
            let glyph = Glyph {
                uv_rect: if gw == 0 || gh == 0 {
                    [0.0; 4]
                } else {
                    pixel_uv(
                        item.x as f32,
                        item.y as f32,
                        gw as f32,
                        gh as f32,
                        atlas_w,
                        atlas_h,
                    )
                },
                width: gw as f32,
                height: gh as f32,
                x_offset: item.metrics.xmin as f32,
                y_offset,
                x_advance: item.metrics.advance_width,
            };
            font.insert_glyph(item.ch, glyph);
            if item.ch == ' ' {
                font.space_advance = item.metrics.advance_width;
            }
        }

        Ok((
            font,
            FontAtlasImage {
                width: atlas_w,
                height: atlas_h,
                rgba8,
            },
        ))
    }

    fn empty(
        line_height: f32,
        base: f32,
        space_advance: f32,
        tex_width: u32,
        tex_height: u32,
    ) -> Self {
        Self {
            ascii: [None; 128],
            extra: FxHashMap::default(),
            kernings: FxHashMap::default(),
            line_height,
            base,
            space_advance,
            case_mode: FontCase::Both,
            tex_width,
            tex_height,
        }
    }

    fn insert_glyph(&mut self, ch: char, glyph: Glyph) {
        if (ch as u32) < 128 {
            self.ascii[ch as usize] = Some(glyph);
        } else {
            self.extra.insert(ch, glyph);
        }
    }

    #[inline]
    fn lookup_raw(&self, ch: char) -> Option<Glyph> {
        if (ch as u32) < 128 {
            self.ascii[ch as usize]
        } else {
            self.extra.get(&ch).copied()
        }
    }

    /// Looks up a glyph respecting the font's `FontCase` restriction.
    #[inline]
    #[must_use]
    pub fn glyph(&self, ch: char) -> Option<Glyph> {
        let mapped = match self.case_mode {
            FontCase::Both => ch,
            FontCase::Upper => ch.to_ascii_uppercase(),
            FontCase::Lower => ch.to_ascii_lowercase(),
        };
        self.lookup_raw(mapped)
    }

    /// Returns the kerning adjustment between two consecutive characters.
    #[inline]
    #[must_use]
    pub fn kerning(&self, prev: char, curr: char) -> f32 {
        if self.kernings.is_empty() {
            return 0.0;
        }
        self.kernings.get(&(prev, curr)).copied().unwrap_or(0.0)
    }

    /// Line height in unscaled font pixels.
    #[inline]
    #[must_use]
    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    /// Baseline offset from line top in unscaled font pixels.
    #[inline]
    #[must_use]
    pub fn base(&self) -> f32 {
        self.base
    }

    /// Texture width used to normalize glyph UVs.
    #[inline]
    #[must_use]
    pub fn texture_width(&self) -> u32 {
        self.tex_width
    }

    /// Texture height used to normalize glyph UVs.
    #[inline]
    #[must_use]
    pub fn texture_height(&self) -> u32 {
        self.tex_height
    }

    /// Measures `(width, height)` of an ASCII/UTF-8 byte slice with zero heap
    /// allocations.
    #[must_use]
    pub fn measure_bytes(&self, bytes: &[u8], scale: f32, letter_spacing: f32) -> (f32, f32) {
        self.measure_bytes_with_spacing(bytes, scale, letter_spacing, 0.0)
    }

    /// Measures `(width, height)` of a byte slice including custom line spacing,
    /// with zero heap allocations.
    #[must_use]
    pub fn measure_bytes_with_spacing(
        &self,
        bytes: &[u8],
        scale: f32,
        letter_spacing: f32,
        line_spacing: f32,
    ) -> (f32, f32) {
        if bytes.is_empty() {
            return (0.0, 0.0);
        }

        let mut max_w = 0.0f32;
        let mut lines = 0usize;

        for line in bytes.split(|&b| b == b'\n') {
            lines += 1;
            let line_w = self.measure_line_bytes(line, scale, letter_spacing);
            if line_w > max_w {
                max_w = line_w;
            }
        }

        let step_y = self.line_height * scale + line_spacing;
        let height = self.line_height * scale + (lines.saturating_sub(1) as f32) * step_y;
        (max_w, height)
    }

    /// Measures `(width, height)` of a `&str` with zero heap allocations.
    #[inline]
    #[must_use]
    pub fn measure_str(&self, text: &str, scale: f32, letter_spacing: f32) -> (f32, f32) {
        self.measure_bytes(text.as_bytes(), scale, letter_spacing)
    }

    /// Lays out a stack-formatted byte slice (ASCII or valid UTF-8) into
    /// positioned [`GlyphQuad`]s with **zero heap allocations**.
    pub fn layout_bytes<F>(
        &self,
        bytes: &[u8],
        origin_x: f32,
        origin_y: f32,
        style: &TextStyle,
        mut emit: F,
    ) where
        F: FnMut(GlyphQuad),
    {
        let scale = style.scale;
        let step_y = self.line_height * scale + style.line_spacing;
        let mut cursor_y = origin_y;

        for line in bytes.split(|&b| b == b'\n') {
            let line_offset_x = match style.align {
                TextAlign::Left => 0.0,
                TextAlign::Center => {
                    -0.5 * self.measure_line_bytes(line, scale, style.letter_spacing)
                }
                TextAlign::Right => -self.measure_line_bytes(line, scale, style.letter_spacing),
            };
            let mut cursor_x = origin_x + line_offset_x;
            let mut prev_char: Option<char> = None;

            for ch in Utf8ByteChars::new(line) {
                if ch == '\r' {
                    continue;
                }
                if let Some(prev) = prev_char {
                    cursor_x += self.kerning(prev, ch) * scale + style.letter_spacing;
                }

                if ch == ' ' {
                    let adv = self
                        .glyph(' ')
                        .map_or(self.space_advance, |g| g.x_advance);
                    cursor_x += adv * scale;
                    prev_char = Some(' ');
                    continue;
                }

                if let Some(glyph) = self.glyph(ch) {
                    if glyph.width > 0.0 && glyph.height > 0.0 {
                        emit(GlyphQuad {
                            x: cursor_x + glyph.x_offset * scale,
                            y: cursor_y + glyph.y_offset * scale,
                            width: glyph.width * scale,
                            height: glyph.height * scale,
                            uv_rect: glyph.uv_rect,
                        });
                    }
                    cursor_x += glyph.x_advance * scale;
                } else {
                    cursor_x += self.space_advance * scale;
                }
                prev_char = Some(ch);
            }

            cursor_y += step_y;
        }
    }

    /// Lays out a `&str` into positioned [`GlyphQuad`]s with **zero heap
    /// allocations**.
    #[inline]
    pub fn layout_str<F>(
        &self,
        text: &str,
        origin_x: f32,
        origin_y: f32,
        style: &TextStyle,
        emit: F,
    ) where
        F: FnMut(GlyphQuad),
    {
        self.layout_bytes(text.as_bytes(), origin_x, origin_y, style, emit);
    }

    /// Queues a stack-formatted byte slice directly into `batcher` using
    /// `FilterMode::Nearest` and `BlendMode::Normal` with zero heap
    /// allocations.
    #[inline]
    pub fn draw_bytes(
        &self,
        batcher: &mut SpriteBatcher,
        texture: TextureHandle,
        bytes: &[u8],
        origin_x: f32,
        origin_y: f32,
        style: &TextStyle,
    ) {
        self.draw_bytes_with_filter(
            batcher,
            texture,
            FilterMode::Nearest,
            bytes,
            [origin_x, origin_y],
            style,
        );
    }

    /// Queues a stack-formatted byte slice directly into `batcher` with an
    /// explicit [`FilterMode`] and zero heap allocations.
    pub fn draw_bytes_with_filter(
        &self,
        batcher: &mut SpriteBatcher,
        texture: TextureHandle,
        filter: FilterMode,
        bytes: &[u8],
        pos: [f32; 2],
        style: &TextStyle,
    ) {
        let snap = matches!(filter, FilterMode::Nearest);
        self.layout_bytes(bytes, pos[0], pos[1], style, |quad| {
            let (draw_pos, draw_size) = if snap {
                (
                    [quad.x.round(), quad.y.round()],
                    [quad.width.round(), quad.height.round()],
                )
            } else {
                ([quad.x, quad.y], [quad.width, quad.height])
            };
            let instance = SpriteInstance::new(draw_pos, draw_size)
                .with_uv_rect(quad.uv_rect)
                .with_color(style.color);
            batcher.push(texture, BlendMode::Normal, filter, instance);
        });
    }

    /// Queues a stack-formatted byte slice directly into `batcher` with a crisp
    /// 8-neighbor scaled 1px outline (`FlxTextBorderStyle.OUTLINE`) and zero heap
    /// allocations.
    pub fn draw_bytes_outlined(
        &self,
        batcher: &mut SpriteBatcher,
        texture: TextureHandle,
        bytes: &[u8],
        pos: [f32; 2],
        style: &TextStyle,
        outline_color: [f32; 4],
    ) {
        let mut outline_style = *style;
        outline_style.color = outline_color;
        outline_style.color[3] *= style.color[3];
        let border = style.scale.round().max(1.0);
        const OFFSETS: [[f32; 2]; 8] = [
            [-1.0, -1.0],
            [0.0, -1.0],
            [1.0, -1.0],
            [-1.0, 0.0],
            [1.0, 0.0],
            [-1.0, 1.0],
            [0.0, 1.0],
            [1.0, 1.0],
        ];
        for off in OFFSETS {
            self.draw_bytes(
                batcher,
                texture,
                bytes,
                pos[0] + off[0] * border,
                pos[1] + off[1] * border,
                &outline_style,
            );
        }
        self.draw_bytes(batcher, texture, bytes, pos[0], pos[1], style);
    }

    /// Queues a `&str` directly into `batcher` using `FilterMode::Nearest` and
    /// `BlendMode::Normal` with zero heap allocations.
    #[inline]
    pub fn draw_str(
        &self,
        batcher: &mut SpriteBatcher,
        texture: TextureHandle,
        text: &str,
        origin_x: f32,
        origin_y: f32,
        style: &TextStyle,
    ) {
        self.draw_bytes(batcher, texture, text.as_bytes(), origin_x, origin_y, style);
    }

    /// Queues a `&str` directly into `batcher` with an explicit [`FilterMode`]
    /// and zero heap allocations.
    #[inline]
    pub fn draw_str_with_filter(
        &self,
        batcher: &mut SpriteBatcher,
        texture: TextureHandle,
        filter: FilterMode,
        text: &str,
        pos: [f32; 2],
        style: &TextStyle,
    ) {
        self.draw_bytes_with_filter(batcher, texture, filter, text.as_bytes(), pos, style);
    }

    fn measure_line_bytes(&self, line: &[u8], scale: f32, letter_spacing: f32) -> f32 {
        let mut cursor_x = 0.0f32;
        let mut prev_char: Option<char> = None;
        let mut glyph_count = 0usize;

        for ch in Utf8ByteChars::new(line) {
            if ch == '\r' {
                continue;
            }
            if let Some(prev) = prev_char {
                cursor_x += self.kerning(prev, ch) * scale;
            }
            let adv = if ch == ' ' {
                self.glyph(' ')
                    .map_or(self.space_advance, |g| g.x_advance)
            } else if let Some(g) = self.glyph(ch) {
                g.x_advance
            } else {
                self.space_advance
            };
            cursor_x += adv * scale;
            if glyph_count > 0 {
                cursor_x += letter_spacing;
            }
            glyph_count += 1;
            prev_char = Some(ch);
        }

        cursor_x
    }
}

/// Fixed-capacity stack buffer for formatting dynamic HUD/score text without
/// heap allocations (`core::fmt::Write` directly into `[u8; N]`).
#[derive(Debug, Clone)]
pub struct StackTextBuffer<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for StackTextBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> StackTextBuffer<N> {
    /// Creates an empty stack-allocated text buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    /// Resets length to zero without touching backing storage.
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Returns the number of written bytes.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the buffer contains zero bytes.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the written bytes slice.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Returns the written slice as `&str`.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        // `core::fmt::Write` only writes valid UTF-8; truncation never splits
        // inside a multi-byte sequence because `write_str` checks boundary.
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for StackTextBuffer<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let remaining = N.saturating_sub(self.len);
        if remaining == 0 {
            return Ok(());
        }
        let bytes = s.as_bytes();
        let copy_len = if bytes.len() <= remaining {
            bytes.len()
        } else {
            // Floor to a valid UTF-8 char boundary.
            let mut end = remaining;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            end
        };
        self.buf[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

/// Snaps anti-aliased RGBA8 font pixels to crisp binary alpha (`0` or `255`)
/// around `threshold` so bitmap fonts such as AngelCode `vcr-bmp` render with
/// hard pixel edges.
pub fn crisp_alpha_rgba8(rgba8: &mut [u8], threshold: u8) {
    for px in rgba8.as_chunks_mut::<4>().0 {
        if px[3] >= threshold {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
            px[3] = 255;
        } else {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
        }
    }
}

/// Formats a V-Slice HUD score line (`Score: <n> | Misses: <m> | Accuracy: <p>%`)
/// into a stack buffer with zero heap allocations.
pub fn format_hud_score<const N: usize>(
    out: &mut StackTextBuffer<N>,
    score: i64,
    misses: u32,
    accuracy_pct: f32,
) {
    out.clear();
    let _ = write!(
        out,
        "Score: {score} | Misses: {misses} | Accuracy: {accuracy_pct:.1}%"
    );
}

struct Utf8ByteChars<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Utf8ByteChars<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
}

impl Iterator for Utf8ByteChars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let b0 = *self.bytes.get(self.pos)?;
        if b0 < 0x80 {
            self.pos += 1;
            return Some(b0 as char);
        }

        let tail = &self.bytes[self.pos..];
        let width = if (b0 & 0xE0) == 0xC0 {
            2
        } else if (b0 & 0xF0) == 0xE0 {
            3
        } else if (b0 & 0xF8) == 0xF0 {
            4
        } else {
            self.pos += 1;
            return Some(char::REPLACEMENT_CHARACTER);
        };

        if tail.len() < width {
            self.pos = self.bytes.len();
            return Some(char::REPLACEMENT_CHARACTER);
        }

        match core::str::from_utf8(&tail[..width]) {
            Ok(s) => {
                self.pos += width;
                s.chars().next()
            }
            Err(_) => {
                self.pos += 1;
                Some(char::REPLACEMENT_CHARACTER)
            }
        }
    }
}

struct BmfontKvIter<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> BmfontKvIter<'a> {
    const fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }
}

impl<'a> Iterator for BmfontKvIter<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.src.as_bytes();
        while self.pos < bytes.len() && bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        if self.pos >= bytes.len() {
            return None;
        }

        let key_start = self.pos;
        while self.pos < bytes.len()
            && bytes[self.pos] != b'='
            && !bytes[self.pos].is_ascii_whitespace()
        {
            self.pos += 1;
        }
        let key = &self.src[key_start..self.pos];
        if self.pos >= bytes.len() || bytes[self.pos] != b'=' {
            return self.next();
        }
        self.pos += 1; // skip '='

        let val = if self.pos < bytes.len() && bytes[self.pos] == b'"' {
            self.pos += 1;
            let val_start = self.pos;
            while self.pos < bytes.len() && bytes[self.pos] != b'"' {
                self.pos += 1;
            }
            let slice = &self.src[val_start..self.pos];
            if self.pos < bytes.len() {
                self.pos += 1;
            }
            slice
        } else {
            let val_start = self.pos;
            while self.pos < bytes.len() && !bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            &self.src[val_start..self.pos]
        };

        Some((key, val))
    }
}

#[inline]
fn pixel_uv(x: f32, y: f32, w: f32, h: f32, tex_w: u32, tex_h: u32) -> [f32; 4] {
    let inv_w = 1.0 / tex_w as f32;
    let inv_h = 1.0 / tex_h as f32;
    [x * inv_w, y * inv_h, (x + w) * inv_w, (y + h) * inv_h]
}

fn parse_attr_f32(node: &roxmltree::Node<'_, '_>, name: &str) -> Option<f32> {
    node.attribute(name)?.trim().parse::<f32>().ok()
}

fn parse_attr_u32(node: &roxmltree::Node<'_, '_>, name: &str) -> Option<u32> {
    node.attribute(name)?.trim().parse::<u32>().ok()
}

fn strip_sparrow_frame_suffix(name: &str) -> &str {
    if name.len() > 4 {
        let split = name.len() - 4;
        if name.is_char_boundary(split) && name.as_bytes()[split..].iter().all(u8::is_ascii_digit) {
            return &name[..split];
        }
    }
    name
}

fn sparrow_prefix_to_char(prefix: &str) -> Option<char> {
    match prefix {
        "-andpersand-" => Some('&'),
        "-angry faic-" => Some('😠'),
        "-apostraphie-" => Some('\''),
        "-back slash-" => Some('\\'),
        "-comma-" => Some(','),
        "-dash-" => Some('-'),
        "-down arrow-" => Some('↓'),
        "-end quote-" => Some('”'),
        "-exclamation point-" => Some('!'),
        "-forward slash-" => Some('/'),
        "-greater than-" => Some('>'),
        "-heart-" => Some('♥'),
        "-left arrow-" => Some('←'),
        "-less than-" => Some('<'),
        "-multiply x-" => Some('*'),
        "-period-" => Some('.'),
        "-question mark-" => Some('?'),
        "-right arrow-" => Some('→'),
        "-start quote-" => Some('“'),
        "-up arrow-" => Some('↑'),
        _ => {
            let mut chars = prefix.chars();
            let first = chars.next()?;
            if chars.next().is_none() {
                Some(first)
            } else {
                None
            }
        }
    }
}

/// Compact 5x7 pixel font for printable ASCII `32..=126` (95 glyphs).
/// Each glyph is 7 rows, 5 bits wide (`0b10000` = leftmost pixel).
const EMBEDDED_5X7: [[u8; 7]; 95] = [
    // 32 ' '
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
    // 33 '!'
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
    // 34 '"'
    [0b01010, 0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000],
    // 35 '#'
    [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010],
    // 36 '$'
    [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100],
    // 37 '%'
    [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011],
    // 38 '&'
    [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101],
    // 39 '\''
    [0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000],
    // 40 '('
    [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
    // 41 ')'
    [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
    // 42 '*'
    [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000],
    // 43 '+'
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
    // 44 ','
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
    // 45 '-'
    [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
    // 46 '.'
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100],
    // 47 '/'
    [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
    // 48 '0'
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    // 49 '1'
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    // 50 '2'
    [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
    // 51 '3'
    [0b11111, 0b00010, 0b00100, 0b00110, 0b00001, 0b10001, 0b01110],
    // 52 '4'
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    // 53 '5'
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    // 54 '6'
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    // 55 '7'
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    // 56 '8'
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    // 57 '9'
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
    // 58 ':'
    [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
    // 59 ';'
    [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b01000],
    // 60 '<'
    [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
    // 61 '='
    [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
    // 62 '>'
    [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
    // 63 '?'
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
    // 64 '@'
    [0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110],
    // 65 'A'
    [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
    // 66 'B'
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
    // 67 'C'
    [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
    // 68 'D'
    [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
    // 69 'E'
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
    // 70 'F'
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
    // 71 'G'
    [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
    // 72 'H'
    [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
    // 73 'I'
    [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    // 74 'J'
    [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
    // 75 'K'
    [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
    // 76 'L'
    [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
    // 77 'M'
    [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
    // 78 'N'
    [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
    // 79 'O'
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
    // 80 'P'
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
    // 81 'Q'
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
    // 82 'R'
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
    // 83 'S'
    [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
    // 84 'T'
    [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
    // 85 'U'
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
    // 86 'V'
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
    // 87 'W'
    [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
    // 88 'X'
    [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
    // 89 'Y'
    [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
    // 90 'Z'
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
    // 91 '['
    [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
    // 92 '\\'
    [0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001],
    // 93 ']'
    [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
    // 94 '^'
    [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000],
    // 95 '_'
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
    // 96 '`'
    [0b01000, 0b00100, 0b00010, 0b00000, 0b00000, 0b00000, 0b00000],
    // 97 'a'
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111],
    // 98 'b'
    [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b11110],
    // 99 'c'
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10000, 0b10001, 0b01110],
    // 100 'd'
    [0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b10001, 0b01111],
    // 101 'e'
    [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110],
    // 102 'f'
    [0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000],
    // 103 'g'
    [0b00000, 0b00000, 0b01111, 0b10001, 0b01111, 0b00001, 0b01110],
    // 104 'h'
    [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001],
    // 105 'i'
    [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
    // 106 'j'
    [0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100],
    // 107 'k'
    [0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010],
    // 108 'l'
    [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    // 109 'm'
    [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001],
    // 110 'n'
    [0b00000, 0b00000, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001],
    // 111 'o'
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
    // 112 'p'
    [0b00000, 0b00000, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000],
    // 113 'q'
    [0b00000, 0b00000, 0b01111, 0b10001, 0b01111, 0b00001, 0b00001],
    // 114 'r'
    [0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000],
    // 115 's'
    [0b00000, 0b00000, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110],
    // 116 't'
    [0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110],
    // 117 'u'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101],
    // 118 'v'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
    // 119 'w'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010],
    // 120 'x'
    [0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001],
    // 121 'y'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
    // 122 'z'
    [0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111],
    // 123 '{'
    [0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010],
    // 124 '|'
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
    // 125 '}'
    [0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000],
    // 126 '~'
    [0b00000, 0b01000, 0b10101, 0b00010, 0b00000, 0b00000, 0b00000],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
    }

    #[test]
    fn embedded_fallback_covers_printable_ascii_with_zero_alloc_layout() {
        let (font, atlas) = BitmapFont::embedded_fallback();
        assert_eq!(atlas.width, 128);
        assert_eq!(atlas.height, 60);
        assert_eq!(atlas.rgba8.len(), 128 * 60 * 4);

        for code in 32u8..=126u8 {
            assert!(
                font.glyph(code as char).is_some(),
                "missing ASCII glyph {code}"
            );
        }

        let mut stack = StackTextBuffer::<64>::new();
        format_hud_score(&mut stack, 12345, 2, 98.5);
        assert_eq!(
            stack.as_bytes(),
            b"Score: 12345 | Misses: 2 | Accuracy: 98.5%"
        );

        let style = TextStyle {
            scale: 2.0,
            ..Default::default()
        };
        let mut quads = Vec::with_capacity(64);
        font.layout_bytes(stack.as_bytes(), 10.0, 20.0, &style, |q| quads.push(q));
        assert!(!quads.is_empty());
        approx_eq(quads[0].x, 10.0);
        approx_eq(quads[0].y, 20.0);
        approx_eq(quads[0].width, 10.0);
        approx_eq(quads[0].height, 14.0);
    }

    #[test]
    fn parses_bmfont_xml_and_text() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <font>
            <info face="vcr-bmp" size="16"/>
            <common lineHeight="18" base="14" scaleW="122" scaleH="122" pages="1"/>
            <pages><page id="0" file="vcr-bmp.png"/></pages>
            <chars count="3">
                <char id="32" x="0" y="0" width="0" height="0" xoffset="0" yoffset="0" xadvance="10"/>
                <char id="65" x="27" y="32" width="11" height="14" xoffset="-1" yoffset="2" xadvance="10"/>
                <char id="86" x="36" y="62" width="11" height="14" xoffset="-1" yoffset="2" xadvance="10"/>
            </chars>
            <kernings>
                <kerning first="65" second="86" amount="-2"/>
            </kernings>
        </font>"#;

        let font = BitmapFont::from_bmfont(xml, None, None).expect("parse BMFont XML");
        approx_eq(font.line_height(), 18.0);
        let a = font.glyph('A').expect("glyph A");
        approx_eq(a.width, 11.0);
        approx_eq(a.x_offset, -1.0);
        approx_eq(font.kerning('A', 'V'), -2.0);

        let text = r#"info face="Test" size=20
common lineHeight=24 base=18 scaleW=128 scaleH=128 pages=1
page id=0 file="test.png"
char id=65 x=10 y=20 width=12 height=16 xoffset=1 yoffset=2 xadvance=14
kerning first=65 second=65 amount=-1
"#;
        let text_font = BitmapFont::from_bmfont(text, None, None).expect("parse BMFont text");
        approx_eq(text_font.line_height(), 24.0);
        let ta = text_font.glyph('A').expect("glyph A");
        approx_eq(ta.x_advance, 14.0);
        approx_eq(text_font.kerning('A', 'A'), -1.0);
    }

    #[test]
    fn parses_sparrow_bitmap_font_and_enforces_case() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <TextureAtlas imagePath="bold.png">
            <SubTexture name="-andpersand-0000" x="334" y="2" width="36" height="58" />
            <SubTexture name="-period-0000" x="250" y="227" width="26" height="26" />
            <SubTexture name="A0000" x="92" y="441" width="54" height="67" />
            <SubTexture name="A0001" x="148" y="441" width="54" height="67" />
        </TextureAtlas>"#;

        let font = BitmapFont::from_sparrow_xml(xml, 512, 1024).expect("parse Sparrow font");
        assert!(font.glyph('&').is_some());
        assert!(font.glyph('.').is_some());
        let upper_a = font.glyph('A').expect("A");
        let lower_a = font.glyph('a').expect("lowercase maps to uppercase");
        approx_eq(upper_a.width, lower_a.width);
        approx_eq(upper_a.uv_rect[0], 92.0 / 512.0);
        approx_eq(font.line_height(), 67.0);
    }

    #[test]
    fn alignment_and_multiline_measurement() {
        let (font, _) = BitmapFont::embedded_fallback();
        let (w, h) = font.measure_str("AB\nC", 2.0, 0.0);
        // "AB" is 2 chars * 6px * 2 = 24px. 2 lines: 9*2 + 9*2 = 36px.
        approx_eq(w, 24.0);
        approx_eq(h, 36.0);

        let style = TextStyle {
            scale: 2.0,
            align: TextAlign::Center,
            ..Default::default()
        };
        let mut quads = Vec::with_capacity(8);
        font.layout_str("AB", 100.0, 50.0, &style, |q| quads.push(q));
        assert_eq!(quads.len(), 2);
        // Centered around x=100, line width = 24 -> starts at 88.0.
        approx_eq(quads[0].x, 88.0);
    }

    #[test]
    fn crisp_alpha_thresholds_bmfont_edges() {
        let mut rgba = [
            255, 255, 255, 39, // faint AA fringe -> cleared
            255, 255, 255, 111, // just below threshold -> cleared
            255, 255, 255, 112, // at threshold -> solid white
            255, 255, 255, 234, // solid stroke -> solid white
        ];
        crisp_alpha_rgba8(&mut rgba, DEFAULT_BMFONT_ALPHA_THRESHOLD);
        assert_eq!(
            rgba,
            [
                0, 0, 0, 0, //
                0, 0, 0, 0, //
                255, 255, 255, 255, //
                255, 255, 255, 255,
            ]
        );
    }
}
