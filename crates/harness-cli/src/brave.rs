//! Brave Search API key handling for web search in the CLI.
//!
//! When the agent calls `web_search` without a key configured, the turn renderer
//! flags it and we offer the user a friendly prompt to paste one. The key is set
//! in the process environment (so the rest of the session works immediately) and
//! persisted to `~/.oxen-harness/connection.json` — the same file the desktop app
//! uses — so it carries across runs and between the two front-ends.

use std::io::Write;
use std::path::PathBuf;

use harness_tools::web::BRAVE_API_KEY_ENV;

use crate::theme::Ui;

/// `~/.oxen-harness/connection.json`, shared with the desktop app.
fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".oxen-harness").join("connection.json"))
}

/// At startup, seed `BRAVE_API_KEY` from the saved config when it isn't already
/// set in the environment, so a key configured in either front-end just works.
pub(crate) fn load_into_env() {
    if std::env::var(BRAVE_API_KEY_ENV).is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }
    if let Some(key) = saved_key() {
        if !key.trim().is_empty() {
            std::env::set_var(BRAVE_API_KEY_ENV, key);
        }
    }
}

fn saved_key() -> Option<String> {
    let raw = std::fs::read_to_string(config_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("brave_api_key")?.as_str().map(str::to_string)
}

/// Persist `key` into the shared config, preserving any other settings.
fn persist(key: &str) -> std::io::Result<()> {
    let Some(path) = config_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value["brave_api_key"] = serde_json::Value::String(key.to_string());
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
    std::fs::write(&path, pretty)
}

/// Offer the user a friendly prompt to paste a Brave Search API key after a web
/// search failed for the lack of one. Applies it immediately (env) and persists
/// it. A blank line skips. Called between turns, with the terminal in cooked mode.
pub(crate) fn prompt_after_failed_search(ui: &Ui) {
    println!();
    println!(
        "  {} {}",
        ui.accent("🔑"),
        ui.cream("Web search needs a Brave Search API key.")
    );
    println!(
        "  {}",
        ui.dim("Get a free key at https://brave.com/search/api/ — paste it to enable web search."),
    );
    print!("  {} ", ui.accent("brave key ❯"));
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return;
    }
    let key = line.trim();
    if key.is_empty() {
        println!(
            "  {}",
            ui.dim("Skipped — set BRAVE_API_KEY anytime to enable web search.")
        );
        return;
    }

    std::env::set_var(BRAVE_API_KEY_ENV, key);
    match persist(key) {
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
