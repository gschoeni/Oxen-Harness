//! Desktop [`Style`] — typography and framing — plus the validation that keeps a
//! hand-written or model-generated theme from producing broken UI.
//!
//! The CLI is bound to the terminal's own font, so these fields shape the
//! desktop app only; the store maps them onto CSS tokens.

use serde::{Deserialize, Serialize};

use crate::s;

/// Visual styling for the desktop app — typography and framing. The CLI is
/// bound to the terminal's own font, so these fields shape the desktop UI only;
/// the store maps them onto CSS tokens so the same components can render as a
/// chunky 8-bit trail, a hairline newspaper, a soft Apple-style app, or a neon
/// synth grid. Font families name faces the app bundles (see `pixel.css`):
/// `PixelHead`, `PixelRead`, `Playfair`, `Masthead`, `Orbitron`, `PlexSans`,
/// `PlexMono` — plus system stacks like `-apple-system` and `Georgia`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Style {
    /// CSS font-family stack for the wordmark, headings, and micro-labels.
    pub font_display: String,
    /// CSS font-family stack for body / message text.
    pub font_body: String,
    /// CSS font-family stack for code blocks and status readouts.
    pub font_mono: String,
    /// `"uppercase"` or `"none"` — applied to the wordmark and labels.
    pub display_transform: String,
    /// letter-spacing for display text + labels, e.g. `"0.02em"`, `"-0.01em"`.
    pub display_spacing: String,
    /// Corner radius for cards/inputs, a CSS length, e.g. `"3px"`, `"14px"`.
    pub radius: String,
    /// Border width for framed surfaces, e.g. `"2px"`, `"1px"`.
    pub border_width: String,
    /// Depth treatment: `"pixel"` (hard offset), `"soft"` (blurred), `"glow"`
    /// (neon accent), or `"none"` (flat).
    pub shadow: String,
    /// Hero layout for the empty state: `"pixel"` (a framed retro screen),
    /// `"newspaper"` (a masthead), or `"minimal"` (a clean app splash).
    pub hero: String,
    /// Which scene the `"pixel"` hero draws: `"trail"` (covered wagon), `"grid"`
    /// (synthwave outrun grid), or `"none"`. Ignored by other hero layouts.
    pub scene: String,
}

impl Default for Style {
    /// The Oregon Trail look: pixel display face, hard-edged framing.
    fn default() -> Self {
        Style {
            font_display: s("\"PixelHead\", \"Courier New\", monospace"),
            font_body: s(
                "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Segoe UI\", Inter, Roboto, Helvetica, Arial, sans-serif",
            ),
            font_mono: s("\"PixelRead\", \"SF Mono\", ui-monospace, monospace"),
            display_transform: s("uppercase"),
            display_spacing: s("0.02em"),
            radius: s("3px"),
            border_width: s("2px"),
            shadow: s("pixel"),
            hero: s("pixel"),
            scene: s("trail"),
        }
    }
}

impl Style {
    /// Replace any out-of-range enum value or malformed CSS length with the
    /// default for that field, so a hand-written or model-generated theme can't
    /// quietly produce broken UI (an unknown `shadow`, a `radius` of `"huge"`).
    pub(crate) fn sanitize(&mut self) {
        let d = Style::default();
        clamp(
            &mut self.display_transform,
            &["uppercase", "lowercase", "capitalize", "none"],
            &d.display_transform,
        );
        clamp(
            &mut self.shadow,
            &["pixel", "soft", "glow", "none"],
            &d.shadow,
        );
        clamp(&mut self.hero, &["pixel", "newspaper", "minimal"], &d.hero);
        clamp(&mut self.scene, &["trail", "grid", "none"], &d.scene);
        if !is_css_length(&self.radius) {
            self.radius = d.radius;
        }
        if !is_css_length(&self.border_width) {
            self.border_width = d.border_width;
        }
        if !is_css_length(&self.display_spacing) {
            self.display_spacing = d.display_spacing;
        }
    }
}

/// If `value` isn't one of `allowed`, reset it to `fallback`.
fn clamp(value: &mut String, allowed: &[&str], fallback: &str) {
    if !allowed.contains(&value.as_str()) {
        *value = fallback.to_string();
    }
}

/// Whether `s` is a plausible CSS length: `"0"`, or a (possibly signed/decimal)
/// number followed by a known unit. Deliberately conservative — anything it
/// rejects falls back to a safe default rather than reaching the stylesheet.
fn is_css_length(s: &str) -> bool {
    let s = s.trim();
    if s == "0" {
        return true;
    }
    const UNITS: [&str; 13] = [
        "px", "em", "rem", "%", "vh", "vw", "vmin", "vmax", "ch", "ex", "pt", "cm", "mm",
    ];
    let Some(unit_start) = UNITS
        .iter()
        .filter(|u| s.ends_with(*u))
        .max_by_key(|u| u.len())
        .map(|u| s.len() - u.len())
    else {
        return false;
    };
    let num = &s[..unit_start];
    !num.is_empty() && num.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_length_validator_accepts_lengths_and_rejects_junk() {
        for ok in ["0", "3px", "0.02em", "-0.01em", "100%", "1.5rem"] {
            assert!(is_css_length(ok), "{ok} should be valid");
        }
        for bad in ["", "huge", "px", "10", "3 px", "red"] {
            assert!(!is_css_length(bad), "{bad} should be invalid");
        }
    }
}
