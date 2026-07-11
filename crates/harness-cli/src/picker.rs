//! A reusable, Claude-Code-style interactive picker.
//!
//! Renders a single question as a selectable list: arrow keys move, number
//! keys jump, `space` toggles in multi-select, `enter` confirms, and
//! `esc`/`Ctrl-C` cancels. **Typing starts your own answer**: any other
//! printable character drops into the final "✎" row and edits it inline
//! (backspace deletes, `enter` submits, `esc` clears the draft first) — so a
//! prompt that says "type a name below" just works, with no need to discover
//! the row first.
//!
//! Used both by the agent's `ask_user_question` tool ([`crate::ask`]) and by
//! interactive menus (`/model`, `/theme`, `/location`, …), so every
//! option-taking command behaves identically. Key handling is a pure reducer
//! ([`on_key`]) over [`State`], unit-tested without a terminal. Rendering uses
//! `crossterm` raw mode (cross-platform) with a RAII guard that always
//! restores the terminal. This is blocking, so callers in async contexts
//! should run it on a blocking thread.

use std::io::{self, IsTerminal, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, queue, terminal};

use crate::theme::Ui;

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

/// The question being asked, borrowed for the lifetime of one [`select`] call:
/// the header shown as a `[chip]`, the prompt text, the selectable options, and
/// whether more than one may be chosen. Bundled so the rendering and input
/// helpers all take a single spec instead of the same four arguments.
struct Question<'a> {
    header: &'a str,
    question: &'a str,
    options: &'a [Choice],
    multi: bool,
}

/// Present one question and collect the user's selection.
///
/// Returns the chosen label(s) — or free text typed into the "✎" row — or
/// `None` if the user cancelled or there's no interactive terminal.
pub fn select(
    ui: &Ui,
    header: &str,
    question: &str,
    options: &[Choice],
    multi: bool,
) -> io::Result<Option<Vec<String>>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() || options.is_empty() {
        return Ok(None);
    }
    terminal::enable_raw_mode()?;
    let _guard = RawModeGuard;
    let mut out = io::stdout();
    queue!(out, cursor::Hide)?;
    out.flush()?;
    let q = Question {
        header,
        question,
        options,
        multi,
    };
    prompt_one(ui, &mut out, &q)
}

/// Restores cooked mode + the cursor when the picker exits (any path).
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), cursor::Show);
    }
}

/// The picker's interactive state: the focused row (options first, the "✎"
/// free-text row last), multi-select checkmarks, and the inline typed draft.
struct State {
    cursor: usize,
    checked: Vec<bool>,
    input: String,
}

impl State {
    fn new(options: usize) -> Self {
        Self {
            cursor: 0,
            checked: vec![false; options],
            input: String::new(),
        }
    }
}

/// What a key did to the picker.
enum Outcome {
    /// State may have changed; repaint and keep reading keys.
    Continue,
    /// The user backed out (esc with no draft / Ctrl-C).
    Cancel,
    /// A final selection: option label(s) and/or the typed answer.
    Submit(Vec<String>),
}

/// Apply one key to the picker state — a pure reducer, so the interaction
/// rules are testable without a terminal.
///
/// Focus rules: while an *option* row is focused, `1-9` jump (and select in
/// single-choice), `space` toggles in multi-select, and any other printable
/// character moves focus to the "✎" row and starts the draft. While the "✎"
/// row is focused, every printable character (including digits and spaces)
/// edits the draft, so answers like "1848" or "santa fe" type naturally.
fn on_key(q: &Question, s: &mut State, key: KeyEvent) -> Outcome {
    let custom_row = q.options.len();
    let rows = q.options.len() + 1;
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Outcome::Cancel,
        KeyCode::Up => {
            s.cursor = (s.cursor + rows - 1) % rows;
            Outcome::Continue
        }
        KeyCode::Down | KeyCode::Tab => {
            s.cursor = (s.cursor + 1) % rows;
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

fn prompt_one(ui: &Ui, out: &mut io::Stdout, q: &Question) -> io::Result<Option<Vec<String>>> {
    let mut state = State::new(q.options.len());
    let mut prev_lines = 0usize;

    loop {
        let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
        let lines = render(ui, q, &state, width);
        prev_lines = draw_block(out, &lines, prev_lines)?;

        let Some(key) = read_key()? else { continue };
        match on_key(q, &mut state, key) {
            Outcome::Continue => {}
            Outcome::Cancel => return Ok(None),
            Outcome::Submit(selected) => {
                finish(ui, out, q, &selected, prev_lines)?;
                return Ok(Some(selected));
            }
        }
    }
}

fn checked_labels(options: &[Choice], checked: &[bool]) -> Vec<String> {
    options
        .iter()
        .zip(checked)
        .filter(|(_, &on)| on)
        .map(|(o, _)| o.label.clone())
        .collect()
}

/// Render the question as a framed "card" so it stands out from the surrounding
/// conversation: an accent top rule labelled with the question's header, an
/// accent bar down the left of every line, and a matching bottom rule.
///
/// Every returned line must fit in `width` terminal columns: the redraw
/// ([`draw_block`]) moves the cursor up by the *line count*, so a line the
/// terminal soft-wraps would throw the count off and smear stale rows into the
/// scrollback on every repaint. The question is word-wrapped, long option
/// labels truncated, and a long typed draft shows its tail.
fn render(ui: &Ui, q: &Question, s: &State, width: usize) -> Vec<String> {
    let (top, bottom) = card_rules(ui, q.header, width);
    let bar = ui.accent("│");
    let mut lines = vec![top, format!("  {bar}")];

    // "  │  " prefix is 5 columns; leave a right margin so the last glyph
    // never touches the edge (some terminals wrap on the final column).
    let text_width = width.saturating_sub(7);
    for segment in wrap(q.question, text_width) {
        lines.push(format!("  {bar}  {}", ui.strong(&segment)));
    }
    lines.push(format!("  {bar}"));

    for (i, opt) in q.options.iter().enumerate() {
        let active = i == s.cursor;
        let pointer = if active { ui.accent("❯") } else { " ".into() };
        let marker_text = if q.multi {
            if s.checked[i] { "◉" } else { "◯" }.to_string()
        } else {
            format!("{}.", i + 1)
        };
        let marker = if q.multi && s.checked[i] {
            ui.green(&marker_text)
        } else {
            ui.dim(&marker_text)
        };
        // Rendered as `  │  ❯ 1. label  — desc`: 8 fixed columns + the marker,
        // a 2-column gap, the `— ` desc lead, and a 1-column right margin.
        // label + desc share what's left; the label wins but never all of it.
        let content_budget = width.saturating_sub(13 + marker_text.chars().count());
        let has_desc = !opt.description.trim().is_empty();
        let label_cap = if has_desc {
            content_budget.saturating_sub(12).max(6).min(content_budget)
        } else {
            content_budget
        };
        let label_text = truncate(&opt.label, label_cap.max(1));
        let label = if active {
            ui.strong(&label_text)
        } else {
            ui.cream(&label_text)
        };
        let desc_cap = content_budget.saturating_sub(label_text.chars().count());
        let desc = if has_desc && desc_cap >= 4 {
            ui.dim(&format!("— {}", truncate(&opt.description, desc_cap)))
        } else {
            String::new()
        };
        lines.push(format!("  {bar}  {pointer} {marker} {label}  {desc}"));
    }

    let custom_active = s.cursor == q.options.len();
    let pointer = if custom_active {
        ui.accent("❯")
    } else {
        " ".into()
    };
    lines.push(format!(
        "  {bar}  {pointer}   {}",
        custom_row_text(ui, &s.input, custom_active, text_width.saturating_sub(6)),
    ));

    lines.push(format!("  {bar}"));
    let hint = if q.multi {
        "type your own answer · ↑/↓ move · space toggle · enter submit · esc cancel"
    } else {
        "type your own answer · ↑/↓ move · 1-9 jump · enter select · esc cancel"
    };
    for segment in wrap(hint, text_width) {
        lines.push(format!("  {bar}  {}", ui.dim(&segment)));
    }
    lines.push(bottom);
    lines
}

/// The "✎" row: a placeholder until typing starts, then the draft with a
/// caret. A draft longer than `cap` shows its tail, so the cursor's
/// neighborhood stays visible while editing.
fn custom_row_text(ui: &Ui, input: &str, active: bool, cap: usize) -> String {
    if input.is_empty() {
        return ui.dim("✎ Type your own answer…");
    }
    let cap = cap.max(8);
    let shown: String = if input.chars().count() > cap {
        let tail: String = input
            .chars()
            .skip(input.chars().count() - (cap - 1))
            .collect();
        format!("…{tail}")
    } else {
        input.to_string()
    };
    let caret = if active {
        ui.accent("▏")
    } else {
        String::new()
    };
    format!("{} {}{caret}", ui.dim("✎"), ui.cream(&shown))
}

/// The accent-colored top (labelled with the header) and bottom rules that frame
/// the question card, sized to the terminal width. Shared with other card-style
/// prompts (e.g. the `/auth` masked key entry).
pub(crate) fn card_rules(ui: &Ui, header: &str, width: usize) -> (String, String) {
    let label = if header.trim().is_empty() {
        "Question".to_string()
    } else {
        format!("[{header}]")
    };
    let span = width.saturating_sub(4).clamp(24, 76);
    let head = format!("╭─ {label} ");
    let top = format!(
        "{head}{}",
        "─".repeat(span.saturating_sub(head.chars().count()))
    );
    let bottom = format!("╰{}", "─".repeat(span.saturating_sub(1)));
    (
        format!("  {}", ui.accent(&top)),
        format!("  {}", ui.accent(&bottom)),
    )
}

fn finish(
    ui: &Ui,
    out: &mut io::Stdout,
    q: &Question,
    selected: &[String],
    prev_lines: usize,
) -> io::Result<()> {
    clear_block(out, prev_lines)?;
    print_question_line(ui, out, q.header, q.question)?;
    print_chosen(ui, out, selected)
}

fn print_question_line(
    ui: &Ui,
    out: &mut io::Stdout,
    header: &str,
    question: &str,
) -> io::Result<()> {
    let chip = if header.trim().is_empty() {
        String::new()
    } else {
        format!("{} ", ui.accent(&format!("[{header}]")))
    };
    // Keep the accent bar so the collapsed result still reads as the same card.
    write!(out, "  {} {chip}{}\r\n", ui.accent("│"), ui.cream(question))?;
    out.flush()
}

fn print_chosen(ui: &Ui, out: &mut io::Stdout, selected: &[String]) -> io::Result<()> {
    write!(
        out,
        "  {} {} {}\r\n",
        ui.accent("│"),
        ui.green("✓ chosen:"),
        ui.cream(&selected.join(", "))
    )?;
    out.flush()
}

pub(crate) fn draw_block(out: &mut io::Stdout, lines: &[String], prev: usize) -> io::Result<usize> {
    if prev > 0 {
        queue!(
            out,
            cursor::MoveToPreviousLine(prev as u16),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
    }
    for line in lines {
        write!(out, "{line}\r\n")?;
    }
    out.flush()?;
    Ok(lines.len())
}

pub(crate) fn clear_block(out: &mut io::Stdout, prev: usize) -> io::Result<()> {
    if prev > 0 {
        queue!(
            out,
            cursor::MoveToPreviousLine(prev as u16),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        out.flush()?;
    }
    Ok(())
}

fn read_key() -> io::Result<Option<KeyEvent>> {
    match event::read()? {
        // Windows reports key release events too; act only on presses.
        Event::Key(k) if k.kind == KeyEventKind::Press => Ok(Some(k)),
        _ => Ok(None),
    }
}

/// Truncate by character count, appending `…` when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Word-wrap plain text to `max` characters per line (overlong words are
/// hard-split), so the card's redraw math can count physical rows reliably.
fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(8);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        // A word that can't fit on any line is hard-split at the limit.
        if word_len > max {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let mut chunk = String::new();
            for c in word.chars() {
                if chunk.chars().count() == max {
                    out.push(std::mem::take(&mut chunk));
                }
                chunk.push(c);
            }
            line = chunk;
            continue;
        }
        let line_len = line.chars().count();
        if line_len > 0 && line_len + 1 + word_len > max {
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
    fn render_shows_chip_options_and_pointer() {
        let ui = Ui::plain();
        let opts = options();
        let q = question("Storage", "Which backend?", &opts, false);
        let lines = render(&ui, &q, &State::new(opts.len()), 80);
        let joined = lines.join("\n");
        assert!(joined.contains("[Storage]"));
        assert!(joined.contains("Which backend?"));
        assert!(joined.contains("SQLite"));
        assert!(joined.contains("Type your own answer"));
        assert!(lines
            .iter()
            .any(|l| l.contains('❯') && l.contains("SQLite")));
    }

    #[test]
    fn render_shows_the_typed_draft_with_a_caret() {
        let ui = Ui::plain();
        let opts = options();
        let q = question("Location", "Where?", &opts, false);
        let mut s = State::new(opts.len());
        type_str(&q, &mut s, "fort laramie");
        let lines = render(&ui, &q, &s, 80);
        let pen_row = lines
            .iter()
            .find(|l| l.contains("fort laramie"))
            .expect("the draft should render in the ✎ row");
        assert!(pen_row.contains('▏'), "active draft shows a caret");
        assert!(!lines.join(" ").contains("Type your own answer…"));
    }

    #[test]
    fn render_frames_the_question_as_a_card() {
        let ui = Ui::plain();
        let opts = options();
        let q = question("Storage", "Which backend?", &opts, false);
        let lines = render(&ui, &q, &State::new(opts.len()), 80);
        // Top + bottom rules and a left bar on the content make it a distinct card.
        assert!(lines.first().unwrap().contains("╭─"));
        assert!(lines.last().unwrap().contains('╰'));
        assert!(lines
            .iter()
            .any(|l| l.contains("│") && l.contains("Which backend?")));
    }

    #[test]
    fn multi_select_renders_checkboxes_and_hint() {
        let ui = Ui::plain();
        let opts = options();
        let q = question("Storage", "Which?", &opts, true);
        let mut s = State::new(opts.len());
        s.cursor = 1;
        s.checked[0] = true;
        let lines = render(&ui, &q, &s, 80);
        let joined = lines.join("\n");
        assert!(joined.contains('◉'));
        assert!(joined.contains('◯'));
        assert!(joined.contains("space toggle"));
    }

    #[test]
    fn every_rendered_line_fits_the_terminal_width() {
        // A soft-wrapped line breaks the redraw math (MoveToPreviousLine counts
        // logical lines), smearing stale card rows into the scrollback on every
        // repaint. Long questions must wrap, long labels truncate, and a long
        // draft shows only its tail.
        let ui = Ui::plain();
        let opts = vec![
            Choice::new(
                "A rather long option label that keeps going well past any margin",
                "and a description that is itself long enough to need truncation for sure",
            ),
            Choice::new("Short", "brief"),
        ];
        let long_question = "Where does your trail begin? It shows as the \"Departing\" line \
             on the terminal welcome banner and on the desktop app's hero screen. Type a \
             place below, or pick a row.";
        for width in [40usize, 60, 80, 120] {
            let q = question("Location", long_question, &opts, false);
            let mut s = State::new(opts.len());
            type_str(
                &q,
                &mut s,
                "an extremely long hand-typed answer that would never fit on one row of a narrow terminal",
            );
            let lines = render(&ui, &q, &s, width);
            for line in &lines {
                assert!(
                    line.chars().count() <= width,
                    "line overflows {width} cols: {line:?} ({} chars)",
                    line.chars().count()
                );
            }
            // The question is wrapped, not lost (phrases may split across
            // lines, so check for words from its start and end).
            assert!(lines.iter().any(|l| l.contains("Where does")));
            assert!(lines.iter().any(|l| l.contains("row.")));
        }
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
