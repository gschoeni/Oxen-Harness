//! Inline images in the terminal.
//!
//! Terminals that draw pictures do it through one of two escape protocols:
//! the kitty graphics protocol (kitty, WezTerm, Ghostty, Konsole) or iTerm2's
//! inline-image OSC (iTerm2, and terminals that adopted it). Everything else
//! gets text. Detection is by environment only — a probe costs a round trip
//! per launch, and the one we already do (the kitty *keyboard* protocol)
//! showed what an unanswered probe costs on terminals that stay silent.
//!
//! A picture is placed at the cursor, sized in cells so it never exceeds a
//! bounded box, and followed by enough newlines that the transcript keeps
//! flowing below it. Kitty gets PNG (its `f=100` format); iTerm2 takes the
//! file's own bytes.

use base64::Engine as _;
use std::path::Path;

/// How the terminal accepts images, if at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protocol {
    Kitty,
    Iterm2,
    None,
}

impl Protocol {
    /// Detect from the environment, once per process.
    pub(crate) fn detect() -> Self {
        static DETECTED: std::sync::OnceLock<Protocol> = std::sync::OnceLock::new();
        *DETECTED.get_or_init(|| {
            let var = |k: &str| std::env::var(k).unwrap_or_default();
            // `OXEN_HARNESS_IMAGES=off` is the kill switch; a multiplexer in
            // the way (tmux, screen) would need passthrough wrapping the
            // terminal may not honor, so pictures stay off there too.
            if matches!(
                var("OXEN_HARNESS_IMAGES").as_str(),
                "off" | "0" | "false" | "none"
            ) || !var("TMUX").is_empty()
                || var("TERM").starts_with("screen")
            {
                return Self::None;
            }
            let markers = [
                var("KITTY_WINDOW_ID"),
                var("GHOSTTY_RESOURCES_DIR"),
                var("WEZTERM_PANE"),
                var("WEZTERM_EXECUTABLE"),
            ]
            .join("");
            Self::from_env(
                &var("TERM"),
                &var("TERM_PROGRAM"),
                &markers,
                &format!("{}{}", var("LC_TERMINAL"), var("ITERM_SESSION_ID")),
                "",
            )
        })
    }

    /// The pure classification behind [`Protocol::detect`]: `kitty_window` is
    /// any kitty-protocol terminal's marker variable (kitty, Ghostty,
    /// WezTerm), `lc_terminal` any iTerm2 marker.
    pub(crate) fn from_env(
        term: &str,
        term_program: &str,
        kitty_window: &str,
        lc_terminal: &str,
        wezterm: &str,
    ) -> Self {
        let program = term_program.to_ascii_lowercase();
        if !kitty_window.is_empty()
            || term.contains("kitty")
            || term.contains("ghostty")
            || !wezterm.is_empty()
            || matches!(program.as_str(), "wezterm" | "ghostty" | "konsole")
            || (program == "warpterminal" && !cfg!(windows))
        {
            return Self::Kitty;
        }
        if program == "iterm.app" || lc_terminal.to_ascii_lowercase().contains("iterm") {
            return Self::Iterm2;
        }
        Self::None
    }
}

/// The largest picture drawn inline, in cells.
pub(crate) const MAX_COLS: usize = 48;
pub(crate) const MAX_ROWS: usize = 12;
/// Pixels along the longest edge of the bytes actually transmitted — a
/// thumbnail, not the original.
const THUMB_EDGE: u32 = 640;
/// A terminal cell is about twice as tall as it is wide.
const CELL_ASPECT: f64 = 2.0;

/// Fit a `width`×`height` image into at most `max_cols`×`max_rows` cells,
/// preserving its aspect ratio given cells twice as tall as wide. Never zero.
pub(crate) fn fit_cells(
    width: u32,
    height: u32,
    max_cols: usize,
    max_rows: usize,
) -> (usize, usize) {
    if width == 0 || height == 0 {
        return (1, 1);
    }
    // Rows the image needs at full width, then shrink to the row cap.
    let mut cols = max_cols.max(1) as f64;
    let mut rows = cols * (height as f64 / width as f64) / CELL_ASPECT;
    if rows > max_rows as f64 {
        rows = max_rows as f64;
        cols = rows * CELL_ASPECT * (width as f64 / height as f64);
    }
    (
        (cols.round() as usize).max(1),
        (rows.ceil() as usize).max(1),
    )
}

/// An image ready to draw: the terminal escape sequence plus the rows it
/// occupies, so the caller can advance the cursor past it.
pub(crate) struct Placed {
    pub(crate) sequence: String,
    pub(crate) rows: usize,
}

/// Prepare `path` for inline display within `max_cols`×`max_rows` cells, or
/// `None` when the terminal can't draw it or the file isn't a decodable
/// image. The bytes sent are a thumbnail re-encoded from the file.
pub(crate) fn place(
    path: &Path,
    protocol: Protocol,
    max_cols: usize,
    max_rows: usize,
) -> Option<Placed> {
    if protocol == Protocol::None {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    place_bytes(&bytes, protocol, max_cols, max_rows)
}

/// [`place`] for image bytes already in memory (an attachment about to be
/// sent).
pub(crate) fn place_bytes(
    bytes: &[u8],
    protocol: Protocol,
    max_cols: usize,
    max_rows: usize,
) -> Option<Placed> {
    if protocol == Protocol::None {
        return None;
    }
    let decoded = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (cols, rows) = fit_cells(decoded.width(), decoded.height(), max_cols, max_rows);
    let thumb = if decoded.width().max(decoded.height()) > THUMB_EDGE {
        decoded.resize(
            THUMB_EDGE,
            THUMB_EDGE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };
    let mut png = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut png, image::ImageFormat::Png).ok()?;
    let sequence = match protocol {
        Protocol::Kitty => kitty_sequence(png.get_ref(), cols, rows),
        Protocol::Iterm2 => iterm2_sequence(png.get_ref(), cols, rows),
        Protocol::None => return None,
    };
    Some(Placed { sequence, rows })
}

/// Kitty graphics: transmit-and-place a PNG scaled into `cols`×`rows` cells,
/// leaving the cursor where it was (`C=1`) so the caller controls the flow.
/// Payloads go in 4 KiB chunks, `m=1` on every chunk but the last.
pub(crate) fn kitty_sequence(png: &[u8], cols: usize, rows: usize) -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&str> = data
        .as_bytes()
        .chunks(4096)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    let mut out = String::with_capacity(data.len() + chunks.len() * 40);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < chunks.len() { 1 } else { 0 };
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Ga=T,f=100,t=d,q=2,C=1,c={cols},r={rows},m={more};{chunk}\x1b\\"
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={more};{chunk}\x1b\\"));
        }
    }
    out
}

/// iTerm2 inline image: one OSC 1337 carrying the whole file, sized in cells.
pub(crate) fn iterm2_sequence(bytes: &[u8], cols: usize, rows: usize) -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!(
        "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=1;doNotMoveCursor=1:{data}\x07",
        bytes.len()
    )
}

/// Whether the terminal turns OSC 8 into clickable links. Env-only, like
/// image detection: the terminals that draw pictures all do, plus VS Code,
/// Alacritty and Hyper; multiplexers older than tmux 3.4 don't pass it.
pub(crate) fn hyperlinks_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        if !var("TMUX").is_empty() || var("TERM").starts_with("screen") {
            return false;
        }
        if Protocol::detect() != Protocol::None {
            return true;
        }
        let program = var("TERM_PROGRAM").to_ascii_lowercase();
        matches!(
            program.as_str(),
            "vscode" | "hyper" | "alacritty" | "rio" | "tabby"
        ) || !var("ALACRITTY_WINDOW_ID").is_empty()
            || !var("VSCODE_PID").is_empty()
    })
}

/// Wrap already-styled `text` in an OSC 8 hyperlink to `url`.
pub(crate) fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_prefers_kitty_then_iterm_then_nothing() {
        assert_eq!(
            Protocol::from_env("xterm-kitty", "", "", "", ""),
            Protocol::Kitty
        );
        assert_eq!(
            Protocol::from_env("xterm-256color", "", "3", "", ""),
            Protocol::Kitty
        );
        assert_eq!(
            Protocol::from_env("xterm-256color", "WezTerm", "", "", ""),
            Protocol::Kitty
        );
        assert_eq!(
            Protocol::from_env("xterm-ghostty", "ghostty", "", "", ""),
            Protocol::Kitty
        );
        assert_eq!(
            Protocol::from_env("xterm-256color", "iTerm.app", "", "", ""),
            Protocol::Iterm2
        );
        assert_eq!(
            Protocol::from_env("xterm-256color", "", "", "iTerm2", ""),
            Protocol::Iterm2
        );
        assert_eq!(
            Protocol::from_env("xterm-256color", "Apple_Terminal", "", "", ""),
            Protocol::None
        );
        assert_eq!(Protocol::from_env("dumb", "", "", "", ""), Protocol::None);
    }

    #[test]
    fn fitting_keeps_aspect_with_tall_cells_and_never_hits_zero() {
        // A 16:9 image at 48 cols needs 48*(9/16)/2 = 13.5 → capped to 12 rows, cols shrink.
        let (cols, rows) = fit_cells(1920, 1080, 48, 12);
        assert_eq!(rows, 12);
        assert!((40..=44).contains(&cols), "cols {cols}");
        // A wide banner keeps full width and few rows.
        assert_eq!(fit_cells(2000, 200, 48, 12), (48, 3));
        // A tall portrait is bounded by rows.
        let (cols, rows) = fit_cells(500, 2000, 48, 12);
        assert_eq!(rows, 12);
        assert_eq!(cols, 6);
        assert_eq!(fit_cells(0, 0, 48, 12), (1, 1));
    }

    #[test]
    fn kitty_chunks_are_flagged_and_iterm_carries_the_size() {
        let png = vec![0u8; 10_000];
        let seq = kitty_sequence(&png, 20, 5);
        assert!(seq.starts_with("\x1b_Ga=T,f=100,t=d,q=2,C=1,c=20,r=5,m=1;"));
        assert_eq!(
            seq.matches("\x1b_G").count(),
            4,
            "13.3K of base64 → 4 chunks"
        );
        assert!(seq.ends_with("\x1b\\"));
        assert!(seq.contains("\x1b_Gm=0;"));
        let it = iterm2_sequence(&png, 20, 5);
        assert!(it.starts_with("\x1b]1337;File=inline=1;size=10000;width=20;height=5;"));
        assert!(it.ends_with('\x07'));
    }

    #[test]
    fn placing_a_real_png_yields_rows_and_none_without_a_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.png");
        let img = image::RgbaImage::from_pixel(800, 200, image::Rgba([1, 2, 3, 255]));
        image::DynamicImage::ImageRgba8(img).save(&path).unwrap();
        let placed = place(&path, Protocol::Kitty, 48, 12).unwrap();
        assert_eq!(placed.rows, 6);
        assert!(placed.sequence.starts_with("\x1b_G"));
        assert!(place(&path, Protocol::None, 48, 12).is_none());
        assert!(place(&dir.path().join("missing.png"), Protocol::Kitty, 48, 12).is_none());
    }
}
