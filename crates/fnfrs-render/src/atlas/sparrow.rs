//! SparrowV2 XML sprite sheet parser (`<TextureAtlas><SubTexture .../></TextureAtlas>`).

use std::str::FromStr;

use roxmltree::{Document, Node};

use super::{
    build_frame_index, lookup_prefix, AtlasError, AtlasFrame, FrameLookupIndex, PrefixFrames,
};

/// Parsed SparrowV2 XML texture atlas with pre-indexed exact and prefix lookups.
#[derive(Debug)]
pub struct SparrowAtlas {
    image_path: String,
    width: Option<u32>,
    height: Option<u32>,
    frames: Vec<AtlasFrame>,
    index: FrameLookupIndex,
}

impl SparrowAtlas {
    /// Parses a SparrowV2 XML document string into a [`SparrowAtlas`].
    pub fn from_xml(xml: &str) -> Result<Self, AtlasError> {
        let sanitized;
        let doc = match Document::parse(xml) {
            Ok(d) => d,
            Err(_) if xml.contains('&') => {
                sanitized = sanitize_bare_ampersands(xml);
                Document::parse(&sanitized)?
            }
            Err(e) => return Err(e.into()),
        };
        let root = doc.root_element();
        if !root.has_tag_name("TextureAtlas") {
            return Err(AtlasError::InvalidFormat(format!(
                "expected <TextureAtlas> root element, found <{}>",
                root.tag_name().name()
            )));
        }

        let image_path = root.attribute("imagePath").unwrap_or_default().to_owned();
        let width = parse_opt_u32(root, "width")?;
        let height = parse_opt_u32(root, "height")?;

        let subtexture_count = root
            .children()
            .filter(|n| n.is_element() && n.has_tag_name("SubTexture"))
            .count();
        let mut frames = Vec::with_capacity(subtexture_count);

        for node in root
            .children()
            .filter(|n| n.is_element() && n.has_tag_name("SubTexture"))
        {
            frames.push(parse_subtexture(node)?);
        }

        let index = build_frame_index(&frames, false);

        Ok(Self {
            image_path,
            width,
            height,
            frames,
            index,
        })
    }

    /// Image filename from the `imagePath` attribute on `<TextureAtlas>`.
    #[inline]
    #[must_use]
    pub fn image_path(&self) -> &str {
        &self.image_path
    }

    /// Optional atlas texture width from `<TextureAtlas width="...">`.
    #[inline]
    #[must_use]
    pub fn width(&self) -> Option<u32> {
        self.width
    }

    /// Optional atlas texture height from `<TextureAtlas height="...">`.
    #[inline]
    #[must_use]
    pub fn height(&self) -> Option<u32> {
        self.height
    }

    /// All parsed subtexture frames in XML document order.
    #[inline]
    #[must_use]
    pub fn frames(&self) -> &[AtlasFrame] {
        &self.frames
    }

    /// Total number of subtexture frames.
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

impl FromStr for SparrowAtlas {
    type Err = AtlasError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_xml(s)
    }
}

fn parse_subtexture(node: Node<'_, '_>) -> Result<AtlasFrame, AtlasError> {
    let name = req_attr(node, "SubTexture", "name")?.to_owned();
    let x = req_u32(node, "SubTexture", "x")?;
    let y = req_u32(node, "SubTexture", "y")?;
    let width = req_u32(node, "SubTexture", "width")?;
    let height = req_u32(node, "SubTexture", "height")?;

    let rotated = parse_bool_attr(node, "rotated");
    let flip_x = parse_bool_attr(node, "flipX");
    let flip_y = parse_bool_attr(node, "flipY");

    let frame_x = parse_opt_f32(node, "frameX")?.unwrap_or(0.0);
    let frame_y = parse_opt_f32(node, "frameY")?.unwrap_or(0.0);
    let (default_fw, default_fh) = if rotated {
        (height as f32, width as f32)
    } else {
        (width as f32, height as f32)
    };
    let frame_width = parse_opt_f32(node, "frameWidth")?.unwrap_or(default_fw);
    let frame_height = parse_opt_f32(node, "frameHeight")?.unwrap_or(default_fh);

    Ok(AtlasFrame {
        name,
        x,
        y,
        width,
        height,
        frame_x,
        frame_y,
        frame_width,
        frame_height,
        rotated,
        flip_x,
        flip_y,
    })
}

fn sanitize_bare_ampersands(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() + 16);
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            let rest = &xml[i..];
            if rest.starts_with("&amp;")
                || rest.starts_with("&lt;")
                || rest.starts_with("&gt;")
                || rest.starts_with("&quot;")
                || rest.starts_with("&apos;")
                || rest.starts_with("&#")
            {
                out.push('&');
            } else {
                out.push_str("&amp;");
            }
        } else {
            let ch = xml[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    out
}

fn req_attr<'a>(
    node: Node<'a, '_>,
    element: &'static str,
    attribute: &'static str,
) -> Result<&'a str, AtlasError> {
    node.attribute(attribute)
        .ok_or(AtlasError::MissingAttribute { element, attribute })
}

fn req_u32(
    node: Node<'_, '_>,
    element: &'static str,
    attribute: &'static str,
) -> Result<u32, AtlasError> {
    let raw = req_attr(node, element, attribute)?;
    raw.trim().parse::<u32>().map_err(|_| AtlasError::InvalidInt {
        attribute,
        value: raw.to_owned(),
    })
}

fn parse_opt_u32(node: Node<'_, '_>, attribute: &'static str) -> Result<Option<u32>, AtlasError> {
    match node.attribute(attribute) {
        Some(raw) => raw
            .trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| AtlasError::InvalidInt {
                attribute,
                value: raw.to_owned(),
            }),
        None => Ok(None),
    }
}

fn parse_opt_f32(node: Node<'_, '_>, attribute: &'static str) -> Result<Option<f32>, AtlasError> {
    match node.attribute(attribute) {
        Some(raw) => raw
            .trim()
            .parse::<f32>()
            .map(Some)
            .map_err(|_| AtlasError::InvalidFloat {
                attribute,
                value: raw.to_owned(),
            }),
        None => Ok(None),
    }
}

#[inline]
fn parse_bool_attr(node: Node<'_, '_>, attribute: &'static str) -> bool {
    matches!(
        node.attribute(attribute).map(str::trim),
        Some("true" | "1")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<TextureAtlas imagePath="noteStrumline.png" width="2019" height="810">
  <SubTexture name="confirmDown0001" x="218" y="358" width="226" height="224" frameX="-2" frameY="-2" frameWidth="235" frameHeight="230" />
  <SubTexture name="confirmDown0002" x="444" y="364" width="226" height="224" frameX="-2" frameY="-2" frameWidth="235" frameHeight="230" />
  <SubTexture name="holdCoverBlue0001" x="407" y="242" width="108" height="138" rotated="true" flipX="true" frameX="-94" frameY="-94" frameWidth="300" frameHeight="400" />
  <SubTexture name="staticDown0001" x="1159" y="0" width="157" height="154" />
</TextureAtlas>"#;

    #[test]
    fn parses_sparrow_xml_and_attributes() {
        let atlas = SparrowAtlas::from_xml(SAMPLE_XML).expect("parse sparrow xml");
        assert_eq!(atlas.image_path(), "noteStrumline.png");
        assert_eq!(atlas.width(), Some(2019));
        assert_eq!(atlas.height(), Some(810));
        assert_eq!(atlas.len(), 4);

        let f0 = atlas.frame_by_name("confirmDown0001").expect("confirmDown0001");
        assert_eq!(f0.x, 218);
        assert_eq!(f0.y, 358);
        assert_eq!(f0.width, 226);
        assert_eq!(f0.height, 224);
        assert!((f0.frame_x - (-2.0)).abs() < 1e-6);
        assert!((f0.frame_y - (-2.0)).abs() < 1e-6);
        assert!((f0.frame_width - 235.0).abs() < 1e-6);
        assert!((f0.frame_height - 230.0).abs() < 1e-6);
        assert!(!f0.rotated);

        let uv = f0.uv_rect(2019.0, 810.0);
        assert!((uv[0] - (218.0 / 2019.0)).abs() < 1e-6);
        assert!((uv[1] - (358.0 / 810.0)).abs() < 1e-6);
        assert!((uv[2] - (226.0 / 2019.0)).abs() < 1e-6);
        assert!((uv[3] - (224.0 / 810.0)).abs() < 1e-6);

        let rot = atlas.frame_by_name("holdCoverBlue0001").expect("holdCoverBlue0001");
        assert!(rot.rotated);
        assert!(rot.flip_x);
        assert!(!rot.flip_y);

        let untrimmed = atlas.frame_by_name("staticDown0001").expect("staticDown0001");
        assert!((untrimmed.frame_x - 0.0).abs() < 1e-6);
        assert!((untrimmed.frame_width - 157.0).abs() < 1e-6);
        assert!((untrimmed.frame_height - 154.0).abs() < 1e-6);
    }

    #[test]
    fn prefix_lookup_matches_exact_and_partial_without_alloc() {
        let atlas = SparrowAtlas::from_xml(SAMPLE_XML).expect("parse sparrow xml");
        let confirm: Vec<&str> = atlas
            .frames_by_prefix("confirmDown")
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(confirm, vec!["confirmDown0001", "confirmDown0002"]);

        let partial: Vec<&str> = atlas
            .frames_by_prefix("hold")
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(partial, vec!["holdCoverBlue0001"]);

        assert_eq!(atlas.frames_by_prefix("missing").len(), 0);
    }
}
