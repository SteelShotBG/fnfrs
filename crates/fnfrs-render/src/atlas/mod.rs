//! Texture atlas parsers and animation evaluators for SparrowV2 XML,
//! TexturePacker `.txt`, and Adobe Animate (`Animation.json` + `spritemap*.json`).

pub mod animate;
pub mod packer;
pub mod sparrow;

pub use animate::{AnimateAtlas, AnimateSprite, EvaluatedPart, LabelSpan, SpritemapInfo};
pub use packer::PackerAtlas;
pub use sparrow::SparrowAtlas;

use rustc_hash::FxHashMap;
use std::iter::FusedIterator;
use std::slice::Iter;

/// Errors returned while parsing a SparrowV2, TexturePacker, or Adobe Animate atlas.
#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    /// Failed to parse SparrowV2 XML.
    #[error("XML parse error: {0}")]
    Xml(#[from] roxmltree::Error),
    /// Failed to parse Adobe Animate JSON.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// Structural atlas format error.
    #[error("invalid atlas format: {0}")]
    InvalidFormat(String),
    /// Missing required XML attribute on an atlas element.
    #[error("missing required attribute '{attribute}' on <{element}>")]
    MissingAttribute {
        element: &'static str,
        attribute: &'static str,
    },
    /// Invalid integer attribute in XML.
    #[error("invalid integer in attribute '{attribute}': '{value}'")]
    InvalidInt {
        attribute: &'static str,
        value: String,
    },
    /// Invalid float attribute in XML.
    #[error("invalid float in attribute '{attribute}': '{value}'")]
    InvalidFloat {
        attribute: &'static str,
        value: String,
    },
    /// Malformed line in a TexturePacker `.txt` atlas.
    #[error("packer parse error on line {line}: {reason}")]
    PackerLine { line: usize, reason: String },
}

/// A single subtexture region within a sprite sheet atlas.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasFrame {
    /// Frame name (e.g. `"confirmDown0001"` or `"idle spirit_0"`).
    pub name: String,
    /// Left pixel coordinate on the atlas texture.
    pub x: u32,
    /// Top pixel coordinate on the atlas texture.
    pub y: u32,
    /// Packed subtexture width in atlas pixels.
    pub width: u32,
    /// Packed subtexture height in atlas pixels.
    pub height: u32,
    /// Horizontal trim offset from the untrimmed frame origin.
    pub frame_x: f32,
    /// Vertical trim offset from the untrimmed frame origin.
    pub frame_y: f32,
    /// Full untrimmed frame width in pixels.
    pub frame_width: f32,
    /// Full untrimmed frame height in pixels.
    pub frame_height: f32,
    /// Whether the packed subtexture is rotated 90 degrees counter-clockwise in the sheet.
    pub rotated: bool,
    /// Optional horizontal flip flag from SparrowV2 `flipX`.
    pub flip_x: bool,
    /// Optional vertical flip flag from SparrowV2 `flipY`.
    pub flip_y: bool,
}

impl AtlasFrame {
    /// Computes normalized UV rectangle `[u, v, u_width, v_height]` for a texture of size
    /// `(tex_w, tex_h)`. Returns `[0.0; 4]` if either texture dimension is non-positive.
    #[inline]
    #[must_use]
    pub fn uv_rect(&self, tex_w: f32, tex_h: f32) -> [f32; 4] {
        if tex_w <= 0.0 || tex_h <= 0.0 {
            return [0.0; 4];
        }
        [
            self.x as f32 / tex_w,
            self.y as f32 / tex_h,
            self.width as f32 / tex_w,
            self.height as f32 / tex_h,
        ]
    }

    /// Logical untrimmed frame dimensions `[width, height]`.
    #[inline]
    #[must_use]
    pub fn source_size(&self) -> [f32; 2] {
        if self.frame_width > 0.0 && self.frame_height > 0.0 {
            [self.frame_width, self.frame_height]
        } else if self.rotated {
            [self.height as f32, self.width as f32]
        } else {
            [self.width as f32, self.height as f32]
        }
    }

    /// Draw-space offset `[-frame_x, -frame_y]` to align trimmed pixels inside the full frame.
    #[inline]
    #[must_use]
    pub fn trim_offset(&self) -> [f32; 2] {
        [-self.frame_x, -self.frame_y]
    }
}

/// Zero-allocation iterator over [`AtlasFrame`] entries matching a prefix.
#[derive(Debug, Clone)]
pub struct PrefixFrames<'a> {
    frames: &'a [AtlasFrame],
    indices: Iter<'a, usize>,
}

impl<'a> PrefixFrames<'a> {
    #[inline]
    pub(crate) fn new(frames: &'a [AtlasFrame], indices: &'a [usize]) -> Self {
        Self {
            frames,
            indices: indices.iter(),
        }
    }
}

impl<'a> Iterator for PrefixFrames<'a> {
    type Item = &'a AtlasFrame;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.indices.next().map(|&idx| &self.frames[idx])
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}

impl ExactSizeIterator for PrefixFrames<'_> {}

impl DoubleEndedIterator for PrefixFrames<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.indices.next_back().map(|&idx| &self.frames[idx])
    }
}

impl FusedIterator for PrefixFrames<'_> {}

#[derive(Debug)]
pub(crate) struct FrameLookupIndex {
    pub exact: FxHashMap<String, usize>,
    pub prefixes: FxHashMap<String, Vec<usize>>,
    pub sorted_by_name: Vec<usize>,
}

/// Builds exact-name, digit-stripped prefix, and lexicographically sorted lookup tables at load
/// time so per-frame queries require zero heap allocations.
pub(crate) fn build_frame_index(
    frames: &[AtlasFrame],
    sort_prefix_groups_naturally: bool,
) -> FrameLookupIndex {
    let mut exact = FxHashMap::with_capacity_and_hasher(frames.len(), Default::default());
    let mut prefixes: FxHashMap<String, Vec<usize>> = FxHashMap::default();
    let mut sorted_by_name = Vec::with_capacity(frames.len());

    for (idx, frame) in frames.iter().enumerate() {
        exact.entry(frame.name.clone()).or_insert(idx);
        let prefix = frame.name.trim_end_matches(|c: char| c.is_ascii_digit());
        if let Some(list) = prefixes.get_mut(prefix) {
            list.push(idx);
        } else {
            prefixes.insert(prefix.to_owned(), vec![idx]);
        }
        sorted_by_name.push(idx);
    }

    if sort_prefix_groups_naturally {
        for (prefix, indices) in &mut prefixes {
            let prefix_len = prefix.len();
            indices.sort_by_key(|&idx| {
                let suffix = &frames[idx].name[prefix_len..];
                let num = suffix.parse::<u64>().unwrap_or(0);
                (num, idx)
            });
        }
    }

    sorted_by_name.sort_by(|&a, &b| frames[a].name.cmp(&frames[b].name));

    FrameLookupIndex {
        exact,
        prefixes,
        sorted_by_name,
    }
}

/// Looks up frames matching `prefix` using `O(1)` pre-indexed prefix lookup with an `O(log N)`
/// binary-search fallback on `sorted_by_name`, allocating nothing.
#[inline]
pub(crate) fn lookup_prefix<'a>(
    frames: &'a [AtlasFrame],
    index: &'a FrameLookupIndex,
    prefix: &str,
) -> PrefixFrames<'a> {
    if let Some(indices) = index.prefixes.get(prefix) {
        return PrefixFrames::new(frames, indices.as_slice());
    }

    let sorted = index.sorted_by_name.as_slice();
    let start = sorted.partition_point(|&i| frames[i].name.as_str() < prefix);
    let count = sorted[start..].partition_point(|&i| frames[i].name.starts_with(prefix));
    PrefixFrames::new(frames, &sorted[start..start + count])
}
