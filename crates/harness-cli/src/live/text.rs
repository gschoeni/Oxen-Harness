//! Pure text-rendering helpers for the input box: windowing a line editor to the
//! visible columns, word-wrapping a logical line into visual rows, and building
//! the themed composer prompt. All terminal-free and unit-testable.
//!
//! All column math here is in terminal *cells* (via [`crate::width`]), not
//! chars — a CJK glyph or emoji spans two cells, and windowing by chars would
//! let such lines spill past the composer's right edge.

use super::composer::Composer;
use crate::theme::Ui;
use crate::width::char_width;

/// The window of `chars` visible in `avail` cells around the caret: walk left
/// from the caret filling the budget (so the caret hugs the right edge of a
/// long line), then extend right with whatever budget remains.
fn window(chars: &[char], cursor: usize, avail: usize) -> (usize, usize) {
    let cursor = cursor.min(chars.len());
    let mut budget = avail;
    let mut start = cursor;
    while start > 0 {
        let w = char_width(chars[start - 1]);
        if w > budget {
            break;
        }
        budget -= w;
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() {
        let w = char_width(chars[end]);
        if w > budget {
            break;
        }
        budget -= w;
        end += 1;
    }
    (start, end)
}

/// Render a line editor's text windowed to `avail` columns, optionally drawing a
/// reverse-video caret at the insertion point. Returns the painted body together
/// with its *visible* width (ANSI escapes excluded) so callers that frame it can
/// pad to a fixed cell. Shared by the bottom composer and the inline item editor.
pub(super) fn render_buffer(c: &Composer, avail: usize, caret: bool) -> (String, usize) {
    let chars = c.chars();
    let cursor = c.cursor();
    // Window the buffer so the caret stays visible on long lines.
    let (start, end) = window(chars, cursor, avail);
    let mut body = String::new();
    let mut width = 0;
    for (i, ch) in chars.iter().enumerate().take(end).skip(start) {
        if caret && i == cursor {
            body.push_str("\x1b[7m");
            body.push(*ch);
            body.push_str("\x1b[0m");
        } else {
            body.push(*ch);
        }
        width += char_width(*ch);
    }
    if caret && cursor >= chars.len() {
        body.push_str("\x1b[7m \x1b[0m");
        width += 1;
    }
    (body, width)
}

/// Word-wrap one logical line's chars into rows no wider than `width` cells,
/// breaking after the last space where one fits (hard-splitting an over-long
/// word). Returns each row's chars and the char offset (within the logical
/// line) where it starts, so the caret can be mapped onto a wrapped row. An
/// empty line yields a single empty row.
pub(super) fn wrap_line(chars: &[char], width: usize) -> Vec<(usize, Vec<char>)> {
    let width = width.max(1);
    if chars.is_empty() {
        return vec![(0, Vec::new())];
    }
    let mut rows = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // The furthest hard break within the row's cell budget.
        let mut cells = 0;
        let mut hard = i;
        while hard < chars.len() {
            let w = char_width(chars[hard]);
            if cells + w > width {
                break;
            }
            cells += w;
            hard += 1;
        }
        if hard >= chars.len() {
            rows.push((i, chars[i..].to_vec()));
            break;
        }
        // Prefer breaking just after the last space within the row's width.
        let mut brk = hard;
        let mut j = hard;
        while j > i {
            if chars[j - 1] == ' ' {
                brk = j;
                break;
            }
            j -= 1;
        }
        if brk == i {
            brk = hard; // a single word longer than the row — hard split
        }
        if brk == i {
            brk = i + 1; // one glyph wider than the row — still make progress
        }
        rows.push((i, chars[i..brk].to_vec()));
        i = brk;
    }
    rows
}

/// Render one line of text windowed to `avail` columns, optionally drawing a
/// reverse-video caret at `caret` (a char offset within the line, or one past
/// its end). Returns the painted body and its *visible* width (ANSI excluded)
/// so the box can pad to a fixed cell. Like [`render_buffer`] but for a single
/// line.
pub(super) fn render_text_line(
    chars: &[char],
    caret: Option<usize>,
    avail: usize,
) -> (String, usize) {
    let cur = caret.unwrap_or(0);
    let (start, end) = window(chars, cur, avail);
    let mut body = String::new();
    let mut width = 0;
    for (i, ch) in chars.iter().enumerate().take(end).skip(start) {
        if caret == Some(i) {
            body.push_str("\x1b[7m");
            body.push(*ch);
            body.push_str("\x1b[0m");
        } else {
            body.push(*ch);
        }
        width += char_width(*ch);
    }
    if let Some(c) = caret {
        if c >= chars.len() {
            body.push_str("\x1b[7m \x1b[0m");
            width += 1;
        }
    }
    (body, width)
}

/// The composer prompt as `(plain, styled)`: the plain form measures width, the
/// styled form is what's drawn. Uses the same themed icon + label as the idle
/// prompt so typing looks identical whether or not the agent is working; shows
/// the queue depth when messages are stacked.
pub(super) fn composer_prompt(ui: &Ui, depth: usize) -> (String, String) {
    let v = &ui.theme().voice;
    let (icon, label) = (v.prompt_icon.as_str(), v.prompt_label.as_str());
    if depth > 0 {
        let tag = format!("[{depth} queued]");
        (
            format!("{icon} {tag} {label} "),
            format!(
                "{} {} {} ",
                ui.brown(icon),
                ui.brown(&tag),
                ui.accent(label)
            ),
        )
    } else {
        (
            format!("{icon} {label} "),
            format!("{} {} ", ui.brown(icon), ui.accent(label)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_line_breaks_at_spaces_and_hard_splits_long_words() {
        let chars: Vec<char> = "the quick brown fox".chars().collect();
        let rows: Vec<String> = wrap_line(&chars, 10)
            .into_iter()
            .map(|(_, c)| c.into_iter().collect())
            .collect();
        // Breaks after a space, not mid-word.
        assert_eq!(
            rows,
            vec!["the quick ".to_string(), "brown fox".to_string()]
        );

        // A single word longer than the width hard-splits.
        let long: Vec<char> = "supercalifragilistic".chars().collect();
        let rows = wrap_line(&long, 8);
        assert_eq!(rows[0].1.iter().collect::<String>(), "supercal");
        assert_eq!(rows.len(), 3);

        // An empty line is one empty row (so the caret still has a row).
        assert_eq!(wrap_line(&[], 10).len(), 1);
    }

    #[test]
    fn wrap_line_budgets_wide_chars_by_cells() {
        // Four CJK glyphs are 8 cells: a 6-cell row fits three, not four —
        // wrapping by chars would overflow the composer's right edge.
        let chars: Vec<char> = "日本語字".chars().collect();
        let rows = wrap_line(&chars, 6);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1.iter().collect::<String>(), "日本語");
        assert_eq!(rows[1].1.iter().collect::<String>(), "字");
        // Row starts are char offsets, so the caret maps onto wrapped rows.
        assert_eq!(rows[1].0, 3);
    }

    #[test]
    fn render_text_line_windows_wide_chars_by_cells() {
        let chars: Vec<char> = "日本語字".chars().collect();
        // Caret past the end with a 4-cell window: only the last two glyphs
        // (4 cells) fit, and the trailing caret cell paints beyond them.
        let (body, width) = render_text_line(&chars, Some(4), 4);
        assert!(body.contains("語字"), "tail visible: {body:?}");
        assert!(!body.contains('本'), "head windowed out: {body:?}");
        assert_eq!(width, 5, "two wide glyphs + caret cell");
    }
}
