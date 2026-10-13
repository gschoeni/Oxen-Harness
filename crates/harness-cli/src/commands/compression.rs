//! The `/compression` command.

use anyhow::Result;
use harness_agent::Agent;
use harness_compress::CompressionMode;

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// `/compression [off|audit|on]` — show or switch context compression. With no
/// argument, opens the interactive picker (current mode marked). Applies to
/// this live conversation immediately (`Agent::set_compression_mode`) and
/// persists the preference for new sessions — mirroring the desktop toggle.
pub(crate) fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    let current = agent.compression_mode();
    let choice = match rest {
        Some(arg) => arg,
        None => {
            let mark = |m: CompressionMode| if m == current { "  ← current" } else { "" };
            let options = [
                Choice::new(
                    "off",
                    format!(
                        "send every tool result untouched{}",
                        mark(CompressionMode::Off)
                    ),
                ),
                Choice::new(
                    "audit",
                    format!(
                        "measure what compression would save, change nothing{}",
                        mark(CompressionMode::Audit)
                    ),
                ),
                Choice::new(
                    "on",
                    format!(
                        "compress stale tool output (retrieve_original restores){}",
                        mark(CompressionMode::On)
                    ),
                ),
            ];
            match picker::select(
                ui,
                "Compression",
                &format!("Context compression is `{}` — switch it?", current.as_str()),
                &options,
                false,
            )? {
                Some(sel) => sel.into_iter().next().unwrap_or_default(),
                // Cancelled (or no interactive terminal) — leave it untouched.
                None => return Ok(()),
            }
        }
    };

    let mode = match choice.trim().to_ascii_lowercase().as_str() {
        "off" => CompressionMode::Off,
        "audit" => CompressionMode::Audit,
        "on" => CompressionMode::On,
        other => {
            println!(
                "  {} {}",
                ui.red("✗"),
                ui.dim(&format!(
                    "unknown mode `{other}` — expected off, audit, or on"
                )),
            );
            return Ok(());
        }
    };

    agent.set_compression_mode(mode);
    // Persist for future sessions too; failing to persist still leaves the
    // live session switched.
    let persisted = harness_runtime::compression::set_mode(mode);
    let scope = match persisted {
        Ok(()) => "for this chat and new sessions",
        Err(_) => "for this chat (couldn't persist the preference)",
    };
    println!(
        "  {} {}",
        ui.brown("⊙ compression:"),
        ui.cream(&format!("{} — {scope}", mode.as_str())),
    );
    Ok(())
}
