//! The 24-bit RGB [`Color`] type shared by the palette and style layers.
//!
//! Colors serialize as `#rrggbb` hex strings so theme files stay human-readable
//! and paste-compatible with CSS.

use serde::{Deserialize, Serialize};

/// A 24-bit RGB color. Serializes as a `#rrggbb` hex string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// The `(r, g, b)` tuple used by the ANSI painters.
    pub fn rgb(&self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }

    /// The `#rrggbb` form used in theme files and CSS.
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Parse `#rrggbb` / `rrggbb` (also tolerates the 3-digit `#rgb` shorthand).
    pub fn parse(s: &str) -> Result<Self, String> {
        let h = s.trim().trim_start_matches('#');
        let expand = |c: char| {
            let d = c.to_digit(16).map(|v| v as u8);
            d.map(|v| v * 16 + v)
        };
        match h.len() {
            6 => {
                let n = u32::from_str_radix(h, 16).map_err(|_| format!("bad hex color: {s}"))?;
                Ok(Color::new((n >> 16) as u8, (n >> 8) as u8, n as u8))
            }
            3 => {
                let mut it = h.chars();
                let r = it.next().and_then(expand);
                let g = it.next().and_then(expand);
                let b = it.next().and_then(expand);
                match (r, g, b) {
                    (Some(r), Some(g), Some(b)) => Ok(Color::new(r, g, b)),
                    _ => Err(format!("bad hex color: {s}")),
                }
            }
            _ => Err(format!("expected #rrggbb, got: {s}")),
        }
    }
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_round_trips_through_hex() {
        let c = Color::new(240, 190, 140);
        assert_eq!(c.hex(), "#f0be8c");
        assert_eq!(Color::parse("#f0be8c").unwrap(), c);
        assert_eq!(Color::parse("f0be8c").unwrap(), c);
        assert_eq!(Color::parse("#fff").unwrap(), Color::new(255, 255, 255));
        assert!(Color::parse("nope").is_err());
    }
}
