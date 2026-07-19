//! Display-width measurement for terminal column math.
//!
//! Frame alignment throughout the CLI depends on knowing how many terminal
//! cells a string paints. Counting `char`s gets that wrong as soon as a CJK
//! glyph or emoji (two cells) or a combining mark (zero cells) appears, so
//! every module measures through these helpers instead of `.chars().count()`.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Terminal cells occupied by one char (0 for combining marks and controls).
pub(crate) fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Terminal cells occupied by an escape-free string.
pub(crate) fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Terminal cells a string paints, skipping ANSI CSI escape sequences so
/// styled text measures the same as its plain form.
pub(crate) fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip the escape sequence up to and including its final letter.
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            width += char_width(c);
        }
    }
    width
}

/// Cap a plain string to `max` cells of content, appending `…` when shortened
/// (so the result paints at most `max + 1` cells). Mirrors
/// `harness_core::text::ellipsize`, but measured in cells rather than chars.
pub(crate) fn ellipsize(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    let mut kept = String::new();
    let mut cells = 0;
    for c in s.chars() {
        let w = char_width(c);
        if cells + w > max {
            break;
        }
        cells += w;
        kept.push(c);
    }
    format!("{kept}…")
}

/// Fit a plain string into at most `max` cells *including* the `…` marker —
/// for fixed cells where the result must never overflow the budget.
pub(crate) fn fit(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        s.to_string()
    } else {
        ellipsize(s, max.saturating_sub(1))
    }
}

/// Fit a plain string's *tail* into at most `max` cells, prefixing `…` when
/// the head is cut — for drafts edited at their end, where the cursor's
/// neighborhood must stay visible.
pub(crate) fn fit_tail(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut cells = 0;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        let w = char_width(c);
        if cells + w > budget {
            break;
        }
        cells += w;
        start = i;
    }
    format!("…{}", &s[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_and_zero_width_chars_measure_as_cells() {
        assert_eq!(str_width("abc"), 3);
        assert_eq!(str_width("日本語"), 6); // CJK: two cells each
        assert_eq!(str_width("e\u{301}"), 1); // combining accent: zero cells
        assert_eq!(char_width('日'), 2);
        assert_eq!(char_width('a'), 1);
    }

    #[test]
    fn display_width_skips_ansi_and_counts_cells() {
        assert_eq!(display_width("hi"), 2);
        assert_eq!(display_width("\x1b[1mhi\x1b[0m"), 2);
        assert_eq!(display_width("\x1b[38;2;1;2;3m日\x1b[0m"), 2);
    }

    #[test]
    fn ellipsize_caps_by_cells_not_chars() {
        // Four CJK chars = 8 cells; a 5-cell cap keeps two glyphs (4 cells) —
        // the third would overflow — then appends the marker.
        assert_eq!(ellipsize("日本語字", 5), "日本…");
        assert_eq!(ellipsize("short", 10), "short");
    }

    #[test]
    fn fit_never_exceeds_the_budget() {
        assert_eq!(str_width(&fit("日本語字", 5)), 5);
        assert_eq!(fit("ok", 5), "ok");
    }

    #[test]
    fn fit_tail_keeps_the_end_visible() {
        assert_eq!(fit_tail("abcdef", 4), "…def");
        assert_eq!(fit_tail("abc", 4), "abc");
        // Wide tail chars consume the budget twice as fast.
        assert_eq!(fit_tail("abc日本", 5), "…日本");
    }
}
