//! `-p/--print` — one turn, headless.
//!
//! The scripting surface: no banner, no pricing warm-up, no exit review — just
//! the classic (non-live) renderer streaming the model's reply to stdout, then
//! exit. A failed turn writes the error to stderr and exits non-zero so a shell
//! can branch on it.
//!
//! It reuses [`TurnRenderer`] rather than reimplementing a quiet path, so a
//! piped run shows the same tool lines and reply as a cooked-mode REPL turn.

use std::io::{IsTerminal, Read};

use harness_agent::Agent;

use crate::render::TurnRenderer;
use crate::theme::Ui;

/// Resolve the prompt for a `-p` run: the inline argument, or — when `-p` was
/// given bare and stdin is a pipe — everything on stdin.
///
/// `None` means there is nothing to run: a bare `-p` on a terminal, with no
/// piped input to read.
pub(crate) fn resolve_prompt(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if !arg.is_empty() {
        return Some(arg.to_string());
    }
    if std::io::stdin().is_terminal() {
        return None;
    }
    let mut piped = String::new();
    std::io::stdin().read_to_string(&mut piped).ok()?;
    let piped = piped.trim();
    (!piped.is_empty()).then(|| piped.to_string())
}

/// Run exactly one turn and return the process exit code: `0` when the turn
/// finished, `1` when it failed (the error goes to stderr — stdout stays the
/// model's answer alone).
pub(crate) async fn run(agent: &mut Agent, ui: &Ui, prompt: String) -> i32 {
    // `-p "describe @shot.png"` (or a dropped absolute path) attaches the file
    // exactly as it would in the composer; warnings go to stderr so a piped
    // reply stays clean.
    let (text, attachments, warnings) = crate::attach::extract_attachments(&prompt);
    for warning in &warnings {
        eprintln!("{warning}");
    }
    let mut renderer = TurnRenderer::new(ui.clone());
    renderer.begin_thinking();
    let result = agent
        .run_turn_with_attachments(text, attachments, |event: &harness_agent::AgentEvent| {
            renderer.on_event(event)
        })
        .await;
    renderer.finish();
    match result {
        Ok(_) => {
            println!();
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inline_prompt_wins_over_stdin() {
        // No stdin read at all when the argument carries the prompt — the test
        // process's stdin is never touched.
        assert_eq!(
            resolve_prompt("  summarize the diff  ").as_deref(),
            Some("summarize the diff")
        );
    }
}
