//! A reusable, Claude-Code-style interactive picker.
//!
//! Renders a single question as a selectable list: arrow keys (or `j`/`k`) move,
//! number keys jump, `space` toggles in multi-select, `enter` confirms, and a
//! final "type my own answer" row drops to free text. `esc`/`Ctrl-C` cancels.
//!
//! Used both by the agent's `ask_user_question` tool ([`crate::ask`]) and by
//! interactive menus like theme selection. Rendering uses `crossterm` raw mode
//! (cross-platform) with a RAII guard that always restores the terminal. This
//! is blocking, so callers in async contexts should run it on a blocking thread.

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
/// Returns the chosen label(s) — or free text typed in the "my own answer" row
/// — or `None` if the user cancelled or there's no interactive terminal.
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

fn prompt_one(ui: &Ui, out: &mut io::Stdout, q: &Question) -> io::Result<Option<Vec<String>>> {
    let custom_row = q.options.len(); // synthetic "type my own" row
    let row_count = q.options.len() + 1;
    let mut cursor_row = 0usize;
    let mut checked = vec![false; q.options.len()];
    let mut prev_lines = 0usize;

    loop {
        let lines = render(ui, q, cursor_row, &checked);
        prev_lines = draw_block(out, &lines, prev_lines)?;

        let Some(key) = read_key()? else { continue };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                cursor_row = (cursor_row + row_count - 1) % row_count;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                cursor_row = (cursor_row + 1) % row_count;
            }
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if n < q.options.len() {
                    cursor_row = n;
                    if q.multi {
                        checked[n] = !checked[n];
                    } else {
                        let selected = vec![q.options[n].label.clone()];
                        finish(ui, out, q, &selected, prev_lines)?;
                        return Ok(Some(selected));
                    }
                }
            }
            KeyCode::Char(' ') if q.multi && cursor_row < q.options.len() => {
                checked[cursor_row] = !checked[cursor_row];
            }
            KeyCode::Enter => {
                let selected = if cursor_row == custom_row {
                    match collect_custom(ui, out, q, &checked, prev_lines)? {
                        Some(sel) => sel,
                        None => {
                            prev_lines = 0;
                            continue;
                        }
                    }
                } else if q.multi {
                    let mut sel = checked_labels(q.options, &checked);
                    if sel.is_empty() {
                        sel.push(q.options[cursor_row].label.clone());
                    }
                    sel
                } else {
                    vec![q.options[cursor_row].label.clone()]
                };

                if cursor_row != custom_row {
                    finish(ui, out, q, &selected, prev_lines)?;
                }
                return Ok(Some(selected));
            }
            _ => {}
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

fn collect_custom(
    ui: &Ui,
    out: &mut io::Stdout,
    q: &Question,
    checked: &[bool],
    prev_lines: usize,
) -> io::Result<Option<Vec<String>>> {
    clear_block(out, prev_lines)?;
    print_question_line(ui, out, q.header, q.question)?;

    terminal::disable_raw_mode()?;
    queue!(out, cursor::Show)?;
    write!(out, "  {} ", ui.brown("✎ your answer:"))?;
    out.flush()?;

    let mut line = String::new();
    let read = io::stdin().read_line(&mut line)?;
    terminal::enable_raw_mode()?;
    queue!(out, cursor::Hide)?;
    out.flush()?;

    let typed = line.trim();
    if read == 0 && typed.is_empty() {
        return Ok(None);
    }
    let mut selected = if q.multi {
        checked_labels(q.options, checked)
    } else {
        Vec::new()
    };
    if !typed.is_empty() {
        selected.push(typed.to_string());
    }
    if selected.is_empty() {
        return Ok(None);
    }
    print_chosen(ui, out, &selected)?;
    Ok(Some(selected))
}

/// Render the question as a framed "card" so it stands out from the surrounding
/// conversation: an accent top rule labelled with the question's header, an
/// accent bar down the left of every line, and a matching bottom rule.
fn render(ui: &Ui, q: &Question, cursor_row: usize, checked: &[bool]) -> Vec<String> {
    let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
    let (top, bottom) = card_rules(ui, q.header, width);
    let bar = ui.accent("│");
    let mut lines = vec![top, format!("  {bar}")];

    lines.push(format!("  {bar}  {}", ui.strong(q.question)));
    lines.push(format!("  {bar}"));

    for (i, opt) in q.options.iter().enumerate() {
        let active = i == cursor_row;
        let pointer = if active { ui.accent("❯") } else { " ".into() };
        let marker = if q.multi {
            if checked[i] {
                ui.green("◉")
            } else {
                ui.dim("◯")
            }
        } else {
            ui.dim(&format!("{}.", i + 1))
        };
        let label = if active {
            ui.strong(&opt.label)
        } else {
            ui.cream(&opt.label)
        };
        let desc = if opt.description.trim().is_empty() {
            String::new()
        } else {
            let budget = width.saturating_sub(opt.label.len() + 16);
            ui.dim(&format!("— {}", truncate(&opt.description, budget.max(8))))
        };
        lines.push(format!("  {bar}  {pointer} {marker} {label}  {desc}"));
    }

    let active = cursor_row == q.options.len();
    let pointer = if active { ui.accent("❯") } else { " ".into() };
    lines.push(format!(
        "  {bar}  {pointer}   {}",
        ui.dim("✎ Type my own answer…")
    ));

    lines.push(format!("  {bar}"));
    let hint = if q.multi {
        "↑/↓ move · space toggle · enter submit · esc cancel"
    } else {
        "↑/↓ move · 1-9 jump · enter select · esc cancel"
    };
    lines.push(format!("  {bar}  {}", ui.dim(hint)));
    lines.push(bottom);
    lines
}

/// The accent-colored top (labelled with the header) and bottom rules that frame
/// the question card, sized to the terminal width.
fn card_rules(ui: &Ui, header: &str, width: usize) -> (String, String) {
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

fn draw_block(out: &mut io::Stdout, lines: &[String], prev: usize) -> io::Result<usize> {
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

fn clear_block(out: &mut io::Stdout, prev: usize) -> io::Result<()> {
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

    #[test]
    fn render_shows_chip_options_and_pointer() {
        let ui = Ui::plain();
        let opts = options();
        let q = question("Storage", "Which backend?", &opts, false);
        let lines = render(&ui, &q, 0, &[false, false]);
        let joined = lines.join("\n");
        assert!(joined.contains("[Storage]"));
        assert!(joined.contains("Which backend?"));
        assert!(joined.contains("SQLite"));
        assert!(joined.contains("Type my own answer"));
        assert!(lines
            .iter()
            .any(|l| l.contains('❯') && l.contains("SQLite")));
    }

    #[test]
    fn render_frames_the_question_as_a_card() {
        let ui = Ui::plain();
        let opts = options();
        let q = question("Storage", "Which backend?", &opts, false);
        let lines = render(&ui, &q, 0, &[false, false]);
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
        let lines = render(&ui, &q, 1, &[true, false]);
        let joined = lines.join("\n");
        assert!(joined.contains('◉'));
        assert!(joined.contains('◯'));
        assert!(joined.contains("space toggle"));
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
}
