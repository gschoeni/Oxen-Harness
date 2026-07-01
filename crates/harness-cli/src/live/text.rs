//! Pure text-rendering helpers for the input box: windowing a line editor to the
//! visible columns, word-wrapping a logical line into visual rows, and building
//! the themed composer prompt. All terminal-free and unit-testable.

use super::composer::Composer;
use crate::theme::Ui;

/// Render a line editor's text windowed to `avail` columns, optionally drawing a
/// reverse-video caret at the insertion point. Returns the painted body together
/// with its *visible* width (ANSI escapes excluded) so callers that frame it can
/// pad to a fixed cell. Shared by the bottom composer and the inline item editor.
pub(super) fn render_buffer(c: &Composer, avail: usize, caret: bool) -> (String, usize) {
    let chars = c.chars();
    let cursor = c.cursor();
    // Window the buffer so the caret stays visible on long lines.
    let start = cursor.saturating_sub(avail);
    let end = (start + avail).min(chars.len());
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
        width += 1;
    }
    if caret && cursor >= chars.len() {
        body.push_str("\x1b[7m \x1b[0m");
        width += 1;
    }
    (body, width)
}

/// Word-wrap one logical line's chars into rows no wider than `width`, breaking
/// after the last space where one fits (hard-splitting an over-long word).
/// Returns each row's chars and the column (within the logical line) where it
/// starts, so the caret can be mapped onto a wrapped row. An empty line yields a
/// single empty row.
pub(super) fn wrap_line(chars: &[char], width: usize) -> Vec<(usize, Vec<char>)> {
    let width = width.max(1);
    if chars.is_empty() {
        return vec![(0, Vec::new())];
    }
    let mut rows = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars.len() - i <= width {
            rows.push((i, chars[i..].to_vec()));
            break;
        }
        let hard = i + width;
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
        rows.push((i, chars[i..brk].to_vec()));
        i = brk;
    }
    rows
}

/// Render one line of text windowed to `avail` columns, optionally drawing a
/// reverse-video caret at `caret` (a column within the line, or one past its
/// end). Returns the painted body and its *visible* width (ANSI excluded) so the
/// box can pad to a fixed cell. Like [`render_buffer`] but for a single line.
pub(super) fn render_text_line(
    chars: &[char],
    caret: Option<usize>,
    avail: usize,
) -> (String, usize) {
    let cur = caret.unwrap_or(0);
    let start = cur.saturating_sub(avail);
    let end = (start + avail).min(chars.len());
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
        width += 1;
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
}
