//! TexturePacker `.txt` sprite sheet parser (`<frame_name> = <x> <y> <w> <h>`).

use std::str::FromStr;

use super::{
    build_frame_index, lookup_prefix, AtlasError, AtlasFrame, FrameLookupIndex, PrefixFrames,
};

/// Parsed TexturePacker `.txt` atlas with pre-indexed exact and prefix lookups.
#[derive(Debug)]
pub struct PackerAtlas {
    frames: Vec<AtlasFrame>,
    index: FrameLookupIndex,
}

impl PackerAtlas {
    /// Parses a TexturePacker `.txt` string (ignoring blank lines and `#` comments).
    pub fn from_txt(txt: &str) -> Result<Self, AtlasError> {
        let mut frames = Vec::new();

        for (idx, raw_line) in txt.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let (raw_name, raw_coords) = line.split_once('=').ok_or_else(|| {
                AtlasError::PackerLine {
                    line: line_no,
                    reason: "missing '=' separator".to_owned(),
                }
            })?;

            let name = raw_name.trim();
            if name.is_empty() {
                return Err(AtlasError::PackerLine {
                    line: line_no,
                    reason: "empty frame name".to_owned(),
                });
            }

            let mut coords = raw_coords.split_whitespace();
            let x = parse_coord(&mut coords, line_no, "x")?;
            let y = parse_coord(&mut coords, line_no, "y")?;
            let width = parse_coord(&mut coords, line_no, "w")?;
            let height = parse_coord(&mut coords, line_no, "h")?;

            if coords.next().is_some() {
                return Err(AtlasError::PackerLine {
                    line: line_no,
                    reason: "expected exactly 4 coordinates (x y w h)".to_owned(),
                });
            }

            frames.push(AtlasFrame {
                name: name.to_owned(),
                x,
                y,
                width,
                height,
                frame_x: 0.0,
                frame_y: 0.0,
                frame_width: width as f32,
                frame_height: height as f32,
                rotated: false,
                flip_x: false,
                flip_y: false,
            });
        }

        let index = build_frame_index(&frames, true);

        Ok(Self { frames, index })
    }

    /// All parsed frames in file order.
    #[inline]
    #[must_use]
    pub fn frames(&self) -> &[AtlasFrame] {
        &self.frames
    }

    /// Total number of frames in the atlas.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns `true` if the atlas contains no frames.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Looks up a frame by its exact `name` in `O(1)` time without allocating.
    #[inline]
    #[must_use]
    pub fn frame_by_name(&self, name: &str) -> Option<&AtlasFrame> {
        self.index.exact.get(name).map(|&idx| &self.frames[idx])
    }

    /// Returns a zero-allocation iterator over all frames matching `prefix`.
    #[inline]
    #[must_use]
    pub fn frames_by_prefix(&self, prefix: &str) -> PrefixFrames<'_> {
        lookup_prefix(&self.frames, &self.index, prefix)
    }
}

impl FromStr for PackerAtlas {
    type Err = AtlasError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_txt(s)
    }
}

fn parse_coord<'a>(
    iter: &mut impl Iterator<Item = &'a str>,
    line: usize,
    field: &'static str,
) -> Result<u32, AtlasError> {
    let token = iter.next().ok_or_else(|| AtlasError::PackerLine {
        line,
        reason: format!("missing '{field}' coordinate"),
    })?;
    token.parse::<u32>().map_err(|_| AtlasError::PackerLine {
        line,
        reason: format!("invalid '{field}' integer '{token}'"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TXT: &str = r#"
# Spirit sprite sheet
idle spirit_0 = 0 1028 128 128
idle spirit_1 = 0 1157 128 128
idle spirit_10 = 129 1028 128 128
idle spirit_2 = 1542 0 128 128

  spirit down_0 = 0 0 256 256
"#;

    #[test]
    fn parses_packer_txt_and_sorts_prefix_naturally() {
        let atlas = PackerAtlas::from_txt(SAMPLE_TXT).expect("parse packer txt");
        assert_eq!(atlas.len(), 5);

        let down = atlas.frame_by_name("spirit down_0").expect("spirit down_0");
        assert_eq!(down.x, 0);
        assert_eq!(down.y, 0);
        assert_eq!(down.width, 256);
        assert_eq!(down.height, 256);

        let idle_names: Vec<&str> = atlas
            .frames_by_prefix("idle spirit_")
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            idle_names,
            vec![
                "idle spirit_0",
                "idle spirit_1",
                "idle spirit_2",
                "idle spirit_10",
            ]
        );
    }

    #[test]
    fn rejects_malformed_packer_line() {
        let err = PackerAtlas::from_txt("bad_line_without_equals").unwrap_err();
        assert!(matches!(err, AtlasError::PackerLine { line: 1, .. }));
    }
}
