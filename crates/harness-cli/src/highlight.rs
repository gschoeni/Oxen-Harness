//! Syntax highlighting for fenced code blocks (syntect + two-face).
//!
//! The streaming Markdown renderer feeds one line at a time, which matches
//! syntect's `HighlightLines` model exactly: the highlighter carries parse
//! state across lines, so multi-line constructs (block comments, raw strings)
//! color correctly as they stream. Colors are emitted as 24-bit foreground
//! ANSI only — no backgrounds — so the terminal's own background shows
//! through and a selection of the code still pastes back clean.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style, Theme};
use syntect::parsing::SyntaxSet;

/// The bundled syntax definitions: two-face's curated superset (bat's
/// collection), covering TOML, TypeScript, Dockerfile, and friends that stock
/// syntect lacks. Built once, on first use.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// The highlight palette. A fixed dark scheme rather than the CLI theme's
/// palette: code needs a dozen distinguishable hues, and every CLI theme is
/// authored for prose, not tokens. base16-ocean.dark reads well on the dark
/// terminals the CLI is tuned for.
fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(two_face::theme::EmbeddedThemeName::Base16OceanDark)
            .clone()
    })
}

/// A per-code-block highlighter, created from the fence's language token and
/// fed one line at a time.
pub(crate) struct Highlighter {
    inner: HighlightLines<'static>,
}

impl Highlighter {
    /// A highlighter for a fence's language token (`rust`, `py`, `json`, …),
    /// or `None` when the token names no known language — the caller falls
    /// back to the theme's flat code color.
    pub(crate) fn for_lang(lang: &str) -> Option<Self> {
        let token = lang.trim();
        if token.is_empty() {
            return None;
        }
        let syntax = syntaxes().find_syntax_by_token(token)?;
        Some(Self {
            inner: HighlightLines::new(syntax, theme()),
        })
    }

    /// Highlight one code line into 24-bit ANSI (no trailing newline), or
    /// `None` if highlighting failed (caller falls back to flat color).
    pub(crate) fn line(&mut self, line: &str) -> Option<String> {
        // syntect wants the newline terminator its grammars were built for.
        let with_nl = format!("{line}\n");
        let spans = self.inner.highlight_line(&with_nl, syntaxes()).ok()?;
        let mut out = String::new();
        for (style, text) in spans {
            let text = text.trim_end_matches('\n');
            if text.is_empty() {
                continue;
            }
            out.push_str(&paint_span(style, text));
        }
        out.push_str("\x1b[0m");
        Some(out)
    }
}

/// One styled span as foreground-only SGR: truecolor from the scheme, plus
/// bold/italic where the scheme asks for them.
fn paint_span(style: Style, text: &str) -> String {
    let f = style.foreground;
    let mut sgr = String::from("\x1b[");
    if style.font_style.contains(FontStyle::BOLD) {
        sgr.push_str("1;");
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        sgr.push_str("3;");
    }
    format!("{sgr}38;2;{};{};{}m{text}", f.r, f.g, f.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI escapes, leaving the visible text.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn known_languages_highlight_and_preserve_the_text() {
        let mut hl = Highlighter::for_lang("rust").expect("rust is bundled");
        let line = hl.line("fn main() { let x = 1; }").expect("highlights");
        assert!(line.contains("\x1b[38;2;"), "colored: {line:?}");
        assert!(line.ends_with("\x1b[0m"), "reset at end: {line:?}");
        assert_eq!(plain(&line), "fn main() { let x = 1; }");
    }

    #[test]
    fn state_carries_across_streamed_lines() {
        // A block comment opened on one line still colors the next line —
        // the whole point of keeping HighlightLines across the stream.
        let mut hl = Highlighter::for_lang("rust").unwrap();
        hl.line("/* open").unwrap();
        let inside = hl.line("still a comment").unwrap();
        let mut fresh = Highlighter::for_lang("rust").unwrap();
        let outside = fresh.line("still a comment").unwrap();
        assert_ne!(inside, outside, "comment state must persist across lines");
    }

    #[test]
    fn unknown_language_yields_no_highlighter() {
        assert!(Highlighter::for_lang("not-a-language-xyz").is_none());
        assert!(Highlighter::for_lang("").is_none());
    }

    #[test]
    fn extended_syntaxes_from_two_face_are_available() {
        // TOML ships in bat's collection but not stock syntect — its presence
        // proves the two-face superset is the one actually loaded.
        assert!(Highlighter::for_lang("toml").is_some());
        assert!(Highlighter::for_lang("typescript").is_some());
    }
}
