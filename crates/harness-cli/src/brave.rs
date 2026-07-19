//! Brave Search API key handling for web search in the CLI.
//!
//! When the agent calls `web_search` without a key configured, the turn renderer
//! flags it and we offer the user a friendly prompt to paste one. The key is set
//! in the process environment (so the rest of the session works immediately) and
//! persisted to `~/.oxen-harness/.env` via the shared runtime — the same place
//! the desktop app stores it — so it carries across runs and front-ends.

use std::io::Write;

use crate::theme::Ui;

/// Offer the user a friendly prompt to paste a Brave Search API key after a web
/// search failed for the lack of one. On a terminal this is the shared masked
/// input card (the key is a secret — it never echoes into the scrollback); on
/// a pipe it falls back to a plain line read. Applies the key immediately
/// (env) and persists it. A blank/cancelled entry skips. Called between turns,
/// with the terminal in cooked mode.
pub(crate) fn prompt_after_failed_search(ui: &Ui) {
    println!();
    println!(
        "  {} {}",
        ui.accent("🔑"),
        ui.cream("Web search needs a Brave Search API key.")
    );
    let key = read_brave_key(ui);
    let Some(key) = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) else {
        println!(
            "  {}",
            ui.dim("Skipped — set BRAVE_API_KEY anytime to enable web search.")
        );
        return;
    };

    // set_brave_key persists to .env and sets it in this process.
    match harness_runtime::connection::set_brave_key(&key) {
        Ok(()) => println!(
            "  {} {}",
            ui.green("✓ saved"),
            ui.dim("web search is ready — ask me to search again."),
        ),
        Err(e) => println!(
            "  {} {}",
            ui.green("✓ enabled for this session"),
            ui.dim(&format!("(couldn't persist it: {e})")),
        ),
    }
}

/// The masked card on a TTY; a plain `read_line` for pipes/scripts.
fn read_brave_key(ui: &Ui) -> Option<String> {
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let help = ["Get a free key at https://brave.com/search/api/".to_string()];
        return crate::picker::card_input(
            ui,
            &crate::picker::CardInputSpec {
                header: "Web search",
                title: "Paste your Brave Search API key",
                help: &help,
                prompt: "brave key ❯",
                initial: "",
                mask: true,
                collapse_label: "🔑 Brave Search key:",
            },
        )
        .ok()
        .and_then(crate::picker::CardInput::entered);
    }
    print!("  {} ", ui.accent("brave key ❯"));
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    Some(line)
}
