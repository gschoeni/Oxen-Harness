//! `/auth` — set the Oxen API key from inside the REPL.
//!
//! Opens a masked, picker-style input card (typed characters echo as `•`) so a
//! pasted key never lands in the terminal scrollback or the prompt history. The
//! key is persisted to `~/.oxen-harness/.env` via the shared runtime — the same
//! place the desktop app stores it — and a client carrying the key is swapped
//! into the running agent, so the current conversation authenticates
//! immediately without a restart. `/auth <key>` sets it directly (for scripts),
//! at the cost of the key appearing in the terminal.

use std::io::{self, IsTerminal, Write};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::{cursor, event, queue, terminal};
use harness_agent::Agent;
use harness_llm::OxenClient;

use crate::picker::{card_rules, clear_block, draw_block};
use crate::theme::Ui;

/// Handle `/auth [key]`: take the key inline when given, otherwise read one from
/// the masked prompt card; persist it and authenticate the running agent in
/// place. A blank/cancelled entry leaves everything untouched.
pub(crate) fn handle_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    base_url: &str,
) -> Result<()> {
    let host = harness_llm::host_from_base_url(base_url);
    let key = match rest {
        Some(k) => Some(k),
        None => read_masked_key(ui, &host)?,
    };
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let Some(key) = key else {
        println!(
            "  {}",
            ui.dim("skipped — no key saved. Run /auth anytime to add one.")
        );
        return Ok(());
    };

    // Persist to `.env` + this process (the same store the desktop app uses),
    // then swap a client carrying the key into the running agent — same
    // endpoint, same model, same transcript.
    let persisted = harness_runtime::connection::set_oxen_key(&key);
    agent.set_client(OxenClient::new(base_url, &key, agent.model()));
    match persisted {
        Ok(()) => println!(
            "  {} {}",
            ui.green("🔑 ✓ saved"),
            ui.dim(&format!(
                "authenticated with {host} — this chat picks it up on the next message."
            )),
        ),
        Err(e) => println!(
            "  {} {}",
            ui.green("🔑 ✓ set for this session"),
            ui.dim(&format!("(couldn't persist it: {e})")),
        ),
    }
    Ok(())
}

/// Offer the masked key card at startup, when no API key resolves at all (so
/// the session couldn't even connect). On save the key is persisted the same
/// way `/auth` does; the caller then builds its client with the returned key.
/// Returns `None` when skipped/cancelled or stdin isn't a terminal.
pub(crate) fn prompt_for_missing_key(ui: &Ui, base_url: &str) -> Option<String> {
    let host = harness_llm::host_from_base_url(base_url);
    println!();
    println!(
        "  {} {}",
        ui.accent("🔑"),
        ui.cream(&format!("No Oxen API key found for {host}."))
    );
    let key = read_masked_key(ui, &host).ok().flatten()?;
    match harness_runtime::connection::set_oxen_key(&key) {
        Ok(()) => println!("  {}", ui.green("✓ saved — setting out on the trail…")),
        Err(e) => println!(
            "  {} {}",
            ui.green("✓ set for this session"),
            ui.dim(&format!("(couldn't persist it: {e})")),
        ),
    }
    Some(key)
}

/// A dim one-line nudge to run `/auth`, shown under an authentication failure —
/// `Some` only when `err` looks like an Oxen 401.
pub(crate) fn auth_hint(ui: &Ui, err: &str) -> Option<String> {
    is_auth_error(err).then(|| {
        format!(
            "  {}",
            ui.dim("Your API key is missing or invalid — run /auth to set it.")
        )
    })
}

/// Whether an agent error message reads as an authentication failure.
fn is_auth_error(err: &str) -> bool {
    err.contains("(401)") || err.to_lowercase().contains("must be authenticated")
}

/// Read an API key with masked echo: a picker-style card with a single input
/// row where every character renders as `•`. Enter submits, Esc/Ctrl-C cancels,
/// Backspace deletes, Ctrl-U clears. Pasted text is captured too (bracketed
/// paste), with any stray newlines dropped so a trailing one doesn't submit a
/// half-entered key. Returns `None` when cancelled, empty, or not a terminal.
fn read_masked_key(ui: &Ui, host: &str) -> io::Result<Option<String>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(None);
    }
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    let _ = crossterm::execute!(out, event::EnableBracketedPaste);
    let _guard = MaskedGuard;
    queue!(out, cursor::Hide)?;
    out.flush()?;

    let mut key = String::new();
    let mut prev_lines = 0usize;
    let entered = loop {
        let lines = render_card(ui, host, key.chars().count());
        prev_lines = draw_block(&mut out, &lines, prev_lines)?;

        match event::read()? {
            crossterm::event::Event::Paste(text) => {
                key.extend(text.chars().filter(|c| !c.is_control()));
            }
            crossterm::event::Event::Key(k)
                if k.kind == crossterm::event::KeyEventKind::Press =>
            {
                match k.code {
                    KeyCode::Esc => break None,
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        break None
                    }
                    KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        key.clear()
                    }
                    KeyCode::Backspace => {
                        key.pop();
                    }
                    KeyCode::Enter => break Some(key.trim().to_string()),
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        key.push(c)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    };
    clear_block(&mut out, prev_lines)?;
    // Collapse the card to a single line echoing what happened (never the key).
    let outcome = match &entered {
        Some(k) if !k.is_empty() => ui.dim(&format!("key entered ({} characters)", k.chars().count())),
        _ => ui.dim("cancelled"),
    };
    write!(out, "  {} {} {outcome}\r\n", ui.accent("│"), ui.cream("🔑 Oxen API key:"))?;
    out.flush()?;
    Ok(entered.filter(|k| !k.is_empty()))
}

/// Restores cooked mode, bracketed paste, and the cursor on any exit path.
struct MaskedGuard;

impl Drop for MaskedGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            event::DisableBracketedPaste,
            cursor::Show
        );
    }
}

/// The masked-entry card: where the key will authenticate, where to get one,
/// the bullet-masked input row, and the key hints.
fn render_card(ui: &Ui, host: &str, len: usize) -> Vec<String> {
    let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
    let (top, bottom) = card_rules(ui, "Authenticate", width);
    let bar = ui.accent("│");
    // Cap the echoed bullets so a long key never wraps the row.
    let bullets = "•".repeat(len.min(width.saturating_sub(20)));
    let caret = "\x1b[7m \x1b[0m"; // reverse-video cell as the caret
    vec![
        top,
        format!("  {bar}"),
        format!(
            "  {bar}  {}",
            ui.strong(&format!("Paste your Oxen API key for {host}"))
        ),
        format!(
            "  {bar}  {}",
            ui.dim(&format!(
                "Get one at https://{host}/settings — stored in ~/.oxen-harness/.env"
            ))
        ),
        format!("  {bar}"),
        format!("  {bar}  {} {bullets}{caret}", ui.brown("key ❯")),
        format!("  {bar}"),
        format!(
            "  {bar}  {}",
            ui.dim("enter save · esc cancel · ctrl-u clear")
        ),
        bottom,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_are_recognized() {
        assert!(is_auth_error(
            "Oxen API error (401): You must be authenticated to perform this action."
        ));
        assert!(is_auth_error("You must be authenticated to do that"));
        assert!(!is_auth_error("Oxen API error (500): boom"));
        assert!(!is_auth_error("connection refused"));
    }

    #[test]
    fn hint_only_fires_on_auth_errors() {
        let ui = Ui::plain();
        assert!(auth_hint(&ui, "Oxen API error (401): nope").is_some());
        assert!(auth_hint(&ui, "Oxen API error (429): slow down").is_none());
    }

    #[test]
    fn card_masks_the_key_and_names_the_host() {
        let ui = Ui::plain();
        let lines = render_card(&ui, "hub.oxen.ai", 5);
        let joined = lines.join("\n");
        assert!(joined.contains("hub.oxen.ai"));
        assert!(joined.contains("•••••"));
        assert!(joined.contains("esc cancel"));
        // The card never echoes key characters (only bullets).
        assert!(!joined.contains("sk-"));
    }
}
