//! The card skin and terminal mechanics of the picker: raw-mode ownership,
//! the framed "card" rendering, and the redraw-in-place block drawing shared
//! with the other card-style prompts (`picker::input`).

use std::io::{self, IsTerminal, Write};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::{cursor, queue, terminal};

use crate::theme::Ui;

use super::core::{on_key, truncate, wrap, Choice, Outcome, Question, State};

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
pub(super) struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), cursor::Show);
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
        let content_budget = width.saturating_sub(13 + crate::width::str_width(&marker_text));
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
        let desc_cap = content_budget.saturating_sub(crate::width::str_width(&label_text));
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
    let shown = crate::width::fit_tail(input, cap);
    let caret = if active {
        ui.accent("▏")
    } else {
        String::new()
    };
    format!("{} {}{caret}", ui.dim("✎"), ui.cream(&shown))
}

/// The accent-colored top (labelled with the header) and bottom rules that frame
/// the question card, sized to the terminal width. Shared with the other
/// card-style prompts (`picker::input`).
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
        "─".repeat(span.saturating_sub(crate::width::str_width(&head)))
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

/// Redraw a block of lines in place: move up over the previous frame, clear
/// down, and print the new one — held as one synchronized frame (mode 2026)
/// so the clear is never visible on its own. Returns the new frame's line
/// count.
pub(crate) fn draw_block(out: &mut io::Stdout, lines: &[String], prev: usize) -> io::Result<usize> {
    write!(out, "{}", crate::ansi::SYNC_BEGIN)?;
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
    write!(out, "{}", crate::ansi::SYNC_END)?;
    out.flush()?;
    Ok(lines.len())
}

/// Erase a previously drawn block (see [`draw_block`]).
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

pub(super) fn read_key() -> io::Result<Option<KeyEvent>> {
    match event::read()? {
        // Windows reports key release events too; act only on presses.
        Event::Key(k) if k.kind == KeyEventKind::Press => Ok(Some(k)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::super::core::Outcome;
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
}
