//! The picker's pure layer: the question/state model, the key reducer, and
//! the list-math helpers every selection surface shares (the card picker here,
//! the composer's inline completion picker in `live::completion`).
//!
//! Nothing in this file touches the terminal, so the interaction rules are
//! unit-tested without one.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A selectable option: a short label plus an optional description.
#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub description: String,
}

impl Choice {
    pub fn new(label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
        }
    }
}

/// The question being asked, borrowed for the lifetime of one `select` call:
/// the header shown as a `[chip]`, the prompt text, the selectable options, and
/// whether more than one may be chosen. Bundled so the rendering and input
/// helpers all take a single spec instead of the same four arguments.
pub(super) struct Question<'a> {
    pub(super) header: &'a str,
    pub(super) question: &'a str,
    pub(super) options: &'a [Choice],
    pub(super) multi: bool,
}

/// The picker's interactive state: the focused row (options first, the "✎"
/// free-text row last), multi-select checkmarks, and the inline typed draft.
pub(super) struct State {
    pub(super) cursor: usize,
    pub(super) checked: Vec<bool>,
    pub(super) input: String,
}

impl State {
    pub(super) fn new(options: usize) -> Self {
        Self {
            cursor: 0,
            checked: vec![false; options],
            input: String::new(),
        }
    }
}

/// What a key did to the picker.
pub(super) enum Outcome {
    /// State may have changed; repaint and keep reading keys.
    Continue,
    /// The user backed out (esc with no draft / Ctrl-C).
    Cancel,
    /// A final selection: option label(s) and/or the typed answer.
    Submit(Vec<String>),
}

/// Step `current` by `delta` through a `len`-row list, wrapping at both ends.
/// The one list-walk used by every picker surface.
pub(crate) fn wrap_step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(len as isize) as usize
}

/// The window of rows to show when a `total`-row list is capped at `max`
/// visible: centered on `selected`, clamped to the list's ends.
pub(crate) fn centered_window(selected: usize, total: usize, max: usize) -> std::ops::Range<usize> {
    let max = max.min(total);
    let start = selected
        .saturating_sub(max / 2)
        .min(total.saturating_sub(max));
    start..start + max
}

/// Apply one key to the picker state — a pure reducer, so the interaction
/// rules are testable without a terminal.
///
/// Focus rules: while an *option* row is focused, `1-9` jump (and select in
/// single-choice), `space` toggles in multi-select, and any other printable
/// character moves focus to the "✎" row and starts the draft. While the "✎"
/// row is focused, every printable character (including digits and spaces)
/// edits the draft, so answers like "1848" or "santa fe" type naturally.
pub(super) fn on_key(q: &Question, s: &mut State, key: KeyEvent) -> Outcome {
    let custom_row = q.options.len();
    let rows = q.options.len() + 1;
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Outcome::Cancel,
        KeyCode::Up => {
            s.cursor = wrap_step(s.cursor, -1, rows);
            Outcome::Continue
        }
        KeyCode::Down | KeyCode::Tab => {
            s.cursor = wrap_step(s.cursor, 1, rows);
            Outcome::Continue
        }
        KeyCode::Esc => {
            // First esc discards a typed draft; a second (or with no draft)
            // cancels the picker.
            if s.input.is_empty() {
                Outcome::Cancel
            } else {
                s.input.clear();
                Outcome::Continue
            }
        }
        KeyCode::Backspace => {
            if s.cursor == custom_row {
                s.input.pop();
            }
            Outcome::Continue
        }
        KeyCode::Enter => {
            if s.cursor != custom_row {
                return Outcome::Submit(if q.multi {
                    let mut sel = checked_labels(q.options, &s.checked);
                    if sel.is_empty() {
                        sel.push(q.options[s.cursor].label.clone());
                    }
                    sel
                } else {
                    vec![q.options[s.cursor].label.clone()]
                });
            }
            let typed = s.input.trim().to_string();
            if typed.is_empty() {
                return Outcome::Continue; // nothing drafted yet
            }
            let mut sel = if q.multi {
                checked_labels(q.options, &s.checked)
            } else {
                Vec::new()
            };
            sel.push(typed);
            Outcome::Submit(sel)
        }
        KeyCode::Char(c) => {
            // Shortcuts only while browsing the options; on the "✎" row every
            // character is text.
            if s.cursor != custom_row {
                if let Some(n) = c.to_digit(10).map(|n| n as usize) {
                    if (1..=q.options.len()).contains(&n) {
                        s.cursor = n - 1;
                        if q.multi {
                            s.checked[n - 1] = !s.checked[n - 1];
                            return Outcome::Continue;
                        }
                        return Outcome::Submit(vec![q.options[n - 1].label.clone()]);
                    }
                }
                if c == ' ' && q.multi {
                    s.checked[s.cursor] = !s.checked[s.cursor];
                    return Outcome::Continue;
                }
            }
            s.cursor = custom_row;
            s.input.push(c);
            Outcome::Continue
        }
        _ => Outcome::Continue,
    }
}

pub(super) fn checked_labels(options: &[Choice], checked: &[bool]) -> Vec<String> {
    options
        .iter()
        .zip(checked)
        .filter(|(_, &on)| on)
        .map(|(o, _)| o.label.clone())
        .collect()
}

/// Truncate to `max` terminal cells, appending `…` when shortened.
pub(super) fn truncate(s: &str, max: usize) -> String {
    crate::width::fit(s, max)
}

/// Word-wrap plain text to `max` terminal cells per line (overlong words are
/// hard-split), so the card's redraw math can count physical rows reliably —
/// a soft-wrapped line would smear stale rows into the scrollback.
pub(super) fn wrap(text: &str, max: usize) -> Vec<String> {
    use crate::width::{char_width, str_width};
    let max = max.max(8);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word_w = str_width(word);
        // A word that can't fit on any line is hard-split at the limit.
        if word_w > max {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let mut chunk = String::new();
            let mut cells = 0;
            for c in word.chars() {
                let w = char_width(c);
                if cells + w > max {
                    out.push(std::mem::take(&mut chunk));
                    cells = 0;
                }
                chunk.push(c);
                cells += w;
            }
            line = chunk;
            continue;
        }
        let line_w = str_width(&line);
        if line_w > 0 && line_w + 1 + word_w > max {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<Choice> {
        vec![
            Choice::new("SQLite", "Embedded, zero-config"),
            Choice::new("Postgres", "Server, scales further"),
        ]
    }

    fn question<'a>(
        header: &'a str,
        prompt: &'a str,
        options: &'a [Choice],
        multi: bool,
    ) -> Question<'a> {
        Question {
            header,
            question: prompt,
            options,
            multi,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(q: &Question, s: &mut State, text: &str) {
        for c in text.chars() {
            assert!(matches!(
                on_key(q, s, key(KeyCode::Char(c))),
                Outcome::Continue
            ));
        }
    }

    #[test]
    fn wrap_step_wraps_both_directions() {
        assert_eq!(wrap_step(0, -1, 3), 2);
        assert_eq!(wrap_step(2, 1, 3), 0);
        assert_eq!(wrap_step(1, 1, 3), 2);
        assert_eq!(wrap_step(0, 1, 0), 0); // empty list never divides by zero
    }

    #[test]
    fn centered_window_clamps_to_the_ends() {
        assert_eq!(centered_window(0, 10, 4), 0..4);
        assert_eq!(centered_window(5, 10, 4), 3..7);
        assert_eq!(centered_window(9, 10, 4), 6..10);
        assert_eq!(centered_window(1, 3, 8), 0..3); // max larger than the list
    }

    #[test]
    fn typing_starts_the_custom_answer_and_enter_submits_it() {
        let opts = options();
        let q = question("Location", "Where?", &opts, false);
        let mut s = State::new(opts.len());
        // Just start typing from the option list — no navigation needed.
        type_str(&q, &mut s, "santa fe");
        assert_eq!(s.cursor, opts.len(), "typing focuses the ✎ row");
        assert_eq!(s.input, "santa fe");
        match on_key(&q, &mut s, key(KeyCode::Enter)) {
            Outcome::Submit(sel) => assert_eq!(sel, vec!["santa fe"]),
            _ => panic!("enter should submit the typed answer"),
        }
    }

    #[test]
    fn digits_and_space_type_normally_once_the_pen_row_is_focused() {
        let opts = options();
        let q = question("Location", "Where?", &opts, false);
        let mut s = State::new(opts.len());
        // Focus the ✎ row explicitly, then type an answer full of "shortcut"
        // characters — they must all land in the draft.
        on_key(&q, &mut s, key(KeyCode::Down));
        on_key(&q, &mut s, key(KeyCode::Down));
        assert_eq!(s.cursor, opts.len());
        type_str(&q, &mut s, "1848 k st");
        assert_eq!(s.input, "1848 k st");
    }

    #[test]
    fn backspace_edits_and_esc_clears_then_cancels() {
        let opts = options();
        let q = question("Location", "Where?", &opts, false);
        let mut s = State::new(opts.len());
        type_str(&q, &mut s, "orego");
        on_key(&q, &mut s, key(KeyCode::Backspace));
        assert_eq!(s.input, "oreg");
        // First esc drops the draft but keeps the picker open…
        assert!(matches!(
            on_key(&q, &mut s, key(KeyCode::Esc)),
            Outcome::Continue
        ));
        assert!(s.input.is_empty());
        // …the second cancels.
        assert!(matches!(
            on_key(&q, &mut s, key(KeyCode::Esc)),
            Outcome::Cancel
        ));
    }

    #[test]
    fn enter_on_an_empty_pen_row_does_not_submit() {
        let opts = options();
        let q = question("Location", "Where?", &opts, false);
        let mut s = State::new(opts.len());
        on_key(&q, &mut s, key(KeyCode::Up)); // wraps to the ✎ row
        assert_eq!(s.cursor, opts.len());
        assert!(matches!(
            on_key(&q, &mut s, key(KeyCode::Enter)),
            Outcome::Continue
        ));
    }

    #[test]
    fn digit_jump_still_selects_while_browsing_options() {
        let opts = options();
        let q = question("Storage", "Which?", &opts, false);
        let mut s = State::new(opts.len());
        match on_key(&q, &mut s, key(KeyCode::Char('2'))) {
            Outcome::Submit(sel) => assert_eq!(sel, vec!["Postgres"]),
            _ => panic!("digit should select in single-choice"),
        }
        // An out-of-range digit is just typing.
        let mut s = State::new(opts.len());
        assert!(matches!(
            on_key(&q, &mut s, key(KeyCode::Char('7'))),
            Outcome::Continue
        ));
        assert_eq!(s.input, "7");
    }

    #[test]
    fn enter_picks_the_focused_option_and_multi_combines_checks_with_typed() {
        let opts = options();
        let q = question("Storage", "Which?", &opts, false);
        let mut s = State::new(opts.len());
        match on_key(&q, &mut s, key(KeyCode::Enter)) {
            Outcome::Submit(sel) => assert_eq!(sel, vec!["SQLite"]),
            _ => panic!("enter should pick the focused option"),
        }

        let q = question("Storage", "Which?", &opts, true);
        let mut s = State::new(opts.len());
        on_key(&q, &mut s, key(KeyCode::Char(' '))); // toggle SQLite
        type_str(&q, &mut s, "redis");
        match on_key(&q, &mut s, key(KeyCode::Enter)) {
            Outcome::Submit(sel) => assert_eq!(sel, vec!["SQLite", "redis"]),
            _ => panic!("multi should combine checked options with the typed answer"),
        }
    }

    #[test]
    fn ctrl_c_cancels_even_mid_draft() {
        let opts = options();
        let q = question("Location", "Where?", &opts, false);
        let mut s = State::new(opts.len());
        type_str(&q, &mut s, "half an ans");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(on_key(&q, &mut s, ctrl_c), Outcome::Cancel));
    }

    #[test]
    fn checked_labels_collects_only_selected() {
        let opts = options();
        assert_eq!(checked_labels(&opts, &[false, true]), vec!["Postgres"]);
        assert!(checked_labels(&opts, &[false, false]).is_empty());
    }

    #[test]
    fn truncate_caps_long_descriptions() {
        assert_eq!(truncate("hello", 10), "hello");
        let out = truncate("a very long description here", 10);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 10);
    }

    #[test]
    fn wrap_respects_width_and_keeps_words() {
        assert_eq!(wrap("short line", 40), vec!["short line"]);
        let wrapped = wrap("one two three four five six seven", 9);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 9));
        assert_eq!(wrapped.join(" "), "one two three four five six seven");
        // Overlong words hard-split rather than overflow.
        let split = wrap("supercalifragilistic", 8);
        assert!(split.iter().all(|l| l.chars().count() <= 8));
        assert_eq!(split.concat(), "supercalifragilistic");
        // Empty text still yields one (blank) line.
        assert_eq!(wrap("", 10), vec![""]);
    }
}
