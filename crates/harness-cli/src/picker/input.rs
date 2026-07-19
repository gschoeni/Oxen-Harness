//! A card-framed single-line input prompt — the free-text sibling of the
//! selection card. Used for secrets (`/auth`, the Brave key) with masked echo
//! so a pasted key never lands in the terminal scrollback or prompt history.
//!
//! Enter submits, Esc/Ctrl-C cancels, Backspace deletes, Ctrl-U clears.
//! Pasted text is captured atomically (bracketed paste) with stray newlines
//! dropped, so a trailing one doesn't submit a half-entered value. Blocking,
//! like `picker::select` — call from a blocking context.

use std::io::{self, IsTerminal, Write};

use crossterm::event::{self, KeyCode, KeyModifiers};
use crossterm::{cursor, queue, terminal};

use crate::theme::Ui;

use super::card::{card_rules, clear_block, draw_block};

/// What the input card asks for and how it presents itself.
pub(crate) struct CardInputSpec<'a> {
    /// The `[chip]` label on the card's top rule (e.g. "Authenticate").
    pub(crate) header: &'a str,
    /// The bold ask ("Paste your Oxen API key for hub.oxen.ai").
    pub(crate) title: &'a str,
    /// Dim helper lines under the title (where to get one, where it's stored).
    pub(crate) help: &'a [String],
    /// The input row's prompt label (e.g. "key ❯").
    pub(crate) prompt: &'a str,
    /// A value the card opens pre-filled with — Enter accepts it as-is,
    /// Backspace/Ctrl-U edit it away. Empty opens a blank card.
    pub(crate) initial: &'a str,
    /// Echo typed characters as `•` and never print the value afterwards.
    pub(crate) mask: bool,
    /// The collapsed one-line echo after the card closes (e.g. "🔑 Oxen API
    /// key:") — followed by what happened, never the masked value itself.
    pub(crate) collapse_label: &'a str,
}

/// How the input card closed. Cancel (Esc/Ctrl-C) and skip (Enter on an empty
/// value) are distinct on purpose: a multi-card flow treats cancel as "abort
/// the whole flow" and skip as "leave this piece as it is".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardInput {
    /// Enter on a non-empty value.
    Entered(String),
    /// Enter on an empty value, or stdin/stdout isn't a terminal.
    Skipped,
    /// Esc or Ctrl-C.
    Cancelled,
}

impl CardInput {
    /// The entered value, treating skip and cancel alike (for single-card
    /// callers with no flow to abort).
    pub(crate) fn entered(self) -> Option<String> {
        match self {
            CardInput::Entered(v) => Some(v),
            CardInput::Skipped | CardInput::Cancelled => None,
        }
    }
}

/// Show the card and read one line.
pub(crate) fn card_input(ui: &Ui, spec: &CardInputSpec) -> io::Result<CardInput> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(CardInput::Skipped);
    }
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    let _ = crossterm::execute!(out, event::EnableBracketedPaste);
    let _guard = InputGuard;
    queue!(out, cursor::Hide)?;
    out.flush()?;

    let mut value = spec.initial.to_string();
    let mut prev_lines = 0usize;
    let entered = loop {
        let lines = render_card(ui, spec, &value);
        prev_lines = draw_block(&mut out, &lines, prev_lines)?;

        match event::read()? {
            crossterm::event::Event::Paste(text) => {
                value.extend(text.chars().filter(|c| !c.is_control()));
            }
            crossterm::event::Event::Key(k) if k.kind == crossterm::event::KeyEventKind::Press => {
                match k.code {
                    KeyCode::Esc => break None,
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        break None
                    }
                    KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        value.clear()
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Enter => break Some(value.trim().to_string()),
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        value.push(c)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    };
    clear_block(&mut out, prev_lines)?;
    let result = match entered {
        Some(v) if !v.is_empty() => CardInput::Entered(v),
        Some(_) => CardInput::Skipped,
        None => CardInput::Cancelled,
    };
    // Collapse the card to a single line echoing what happened. A masked value
    // is described by length only; a plain one is echoed back.
    let outcome = match &result {
        CardInput::Entered(v) => {
            if spec.mask {
                ui.dim(&format!("entered ({} characters)", v.chars().count()))
            } else {
                ui.cream(v)
            }
        }
        CardInput::Skipped => ui.dim("skipped"),
        CardInput::Cancelled => ui.dim("cancelled"),
    };
    write!(
        out,
        "  {} {} {outcome}\r\n",
        ui.accent("│"),
        ui.cream(spec.collapse_label)
    )?;
    out.flush()?;
    Ok(result)
}

/// Restores cooked mode, bracketed paste, and the cursor on any exit path.
struct InputGuard;

impl Drop for InputGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), event::DisableBracketedPaste, cursor::Show);
    }
}

/// The input card: title, helper lines, the (possibly bullet-masked) input row
/// with a caret, and the key hints.
fn render_card(ui: &Ui, spec: &CardInputSpec, value: &str) -> Vec<String> {
    let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
    let (top, bottom) = card_rules(ui, spec.header, width);
    let bar = ui.accent("│");
    // Cap the echoed row so a long value never wraps it (soft-wrap breaks the
    // redraw math). Masked: bullets. Plain: the value's tail.
    let cap = width.saturating_sub(20);
    let shown = if spec.mask {
        "•".repeat(value.chars().count().min(cap))
    } else {
        crate::width::fit_tail(value, cap)
    };
    let caret = "\x1b[7m \x1b[0m"; // reverse-video cell as the caret
    let mut lines = vec![top, format!("  {bar}")];
    lines.push(format!("  {bar}  {}", ui.strong(spec.title)));
    for help in spec.help {
        lines.push(format!("  {bar}  {}", ui.dim(help)));
    }
    lines.push(format!("  {bar}"));
    lines.push(format!("  {bar}  {} {shown}{caret}", ui.brown(spec.prompt)));
    lines.push(format!("  {bar}"));
    lines.push(format!(
        "  {bar}  {}",
        ui.dim("enter save · esc cancel · ctrl-u clear")
    ));
    lines.push(bottom);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(help: &'a [String], mask: bool) -> CardInputSpec<'a> {
        CardInputSpec {
            header: "Authenticate",
            title: "Paste your Oxen API key for hub.oxen.ai",
            help,
            prompt: "key ❯",
            initial: "",
            mask,
            collapse_label: "🔑 Oxen API key:",
        }
    }

    #[test]
    fn masked_card_shows_bullets_and_never_the_value() {
        let ui = Ui::plain();
        let help = vec!["Get one at https://hub.oxen.ai/settings".to_string()];
        let lines = render_card(&ui, &spec(&help, true), "sk-abc");
        let joined = lines.join("\n");
        assert!(joined.contains("hub.oxen.ai"));
        assert!(joined.contains("••••••"));
        assert!(joined.contains("esc cancel"));
        assert!(!joined.contains("sk-"), "masked value must never echo");
    }

    #[test]
    fn plain_card_echoes_the_value_and_windows_a_long_one() {
        let ui = Ui::plain();
        let lines = render_card(&ui, &spec(&[], false), "hello");
        assert!(lines.join("\n").contains("hello"));
        let long = "x".repeat(500);
        let lines = render_card(&ui, &spec(&[], false), &long);
        for line in &lines {
            assert!(
                line.chars().count() <= 80,
                "input row must not soft-wrap: {} chars",
                line.chars().count()
            );
        }
    }
}
