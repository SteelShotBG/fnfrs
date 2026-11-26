pub mod character;
pub mod dialogue;
pub mod level;
pub mod notestyle;
pub mod song;
pub mod stage;

use serde::{Deserialize, Deserializer};

/// Deserializes `[x, y]` with fallback to `[0.0, 0.0]`, accepting nulls or short arrays gracefully.
pub(crate) fn deserialize_vec2_zero<'de, D>(deserializer: D) -> Result<[f32; 2], D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<Vec<f32>>::deserialize(deserializer)?;
    Ok(match opt {
        Some(v) if v.len() >= 2 => [v[0], v[1]],
        Some(v) if v.len() == 1 => [v[0], 0.0],
        _ => [0.0, 0.0],
    })
}

/// Deserializes `[x, y]` with fallback to `[1.0, 1.0]`.
pub(crate) fn deserialize_vec2_one<'de, D>(deserializer: D) -> Result<[f32; 2], D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<Vec<f32>>::deserialize(deserializer)?;
    Ok(match opt {
        Some(v) if v.len() >= 2 => [v[0], v[1]],
        Some(v) if v.len() == 1 => [v[0], v[0]],
        _ => [1.0, 1.0],
    })
}

/// Deserializes an optional string, turning `""` into `None`.
pub(crate) fn deserialize_non_empty_opt_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.is_empty()))
}

/// Parses `#RRGGBB`, `#AARRGGBB`, or `0xAARRGGBB` into normalized linear `[r, g, b, a]`.
#[must_use]
pub fn parse_hex_color(hex: &str) -> [f32; 4] {
    let trimmed = hex.trim();
    let digits = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("0x"))
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);

    if digits.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(digits, 16) {
            let r = ((rgb >> 16) & 0xFF) as f32 / 255.0;
            let g = ((rgb >> 8) & 0xFF) as f32 / 255.0;
            let b = (rgb & 0xFF) as f32 / 255.0;
            return [r, g, b, 1.0];
        }
    } else if digits.len() == 8 {
        if let Ok(argb) = u32::from_str_radix(digits, 16) {
            let a = ((argb >> 24) & 0xFF) as f32 / 255.0;
            let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
            let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
            let b = (argb & 0xFF) as f32 / 255.0;
            return [r, g, b, a];
        }
    }

    [1.0, 1.0, 1.0, 1.0]
}
