//! Driving one turn (or a queue of them) through the agent, plus the shared
//! turn-status reporting both terminals use.
//!
//! [`run_turn_and_drain`] is the single entry point the REPL and `/queue run`
//! go through: on a live TTY it hands off to the sticky composer
//! ([`crate::live`]), otherwise it drives the classic [`TurnRenderer`] path
//! and drains stacked messages after. The report helpers ([`retry_notice`],
//! [`turn_failure_lines`], [`context_usage_line`]) live here so the classic
//! prompt and the live composer explain a turn the same way.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;
use harness_agent::Agent;

use crate::queue::MessageQueue;
use crate::render::{truncate, TurnRenderer};
use crate::theme::Ui;
use crate::{attach, brave, commands, live};

/// What to drive through the agent for one turn: a fresh prompt (the normal
/// case), or a continuation of the transcript's dangling last turn — `/retry`
/// after a failure, with no user message re-appended.
#[derive(Debug, Clone)]
pub(crate) enum TurnRequest {
    Prompt(String),
    Continue,
}

/// Whether the transcript stops mid-turn — it ends on a user message (the
/// reply never arrived: a provider error, no internet, a crash) or on a tool
/// result the model never got to react to. Such a session can be continued in
/// place with `/retry` (`Agent::continue_turn`).
pub(crate) fn ends_mid_turn(messages: &[harness_llm::ChatMessage]) -> bool {
    messages
        .last()
        .is_some_and(|m| m.role == "user" || m.role == "tool")
}

/// The one-line notice for a transient model-call failure being retried with
/// backoff — shared by the classic renderer and the live composer.
pub(crate) fn retry_notice(attempt: u32, max_attempts: u32, delay_ms: u64, error: &str) -> String {
    let secs = (delay_ms as f64 / 1000.0).ceil() as u64;
    format!(
        "{error} — retrying in {secs}s (attempt {} of {max_attempts})",
        attempt + 1
    )
}

/// The failure report for a turn that died even after retries: what happened,
/// then how to pick the conversation back up — now (`/retry`, `/model`) or
/// later (`--continue` / `--resume`). Auth failures get the `/auth` hint
/// instead of the generic recovery lines. Shared by the classic prompt and the
/// live composer so both terminals explain the same way out.
pub(crate) fn turn_failure_lines(
    agent: &Agent,
    ui: &Ui,
    err: &harness_agent::AgentError,
) -> Vec<String> {
    let mut lines = vec![
        format!("  {}", ui.red(&ui.death())),
        format!("  {}", ui.dim(&format!("The trail guide says: {err}"))),
    ];
    if let Some(hint) = commands::auth::auth_hint(ui, &err.to_string()) {
        lines.push(hint);
        return lines;
    }
    lines.push(format!(
        "  {}",
        ui.dim("Nothing is lost — every step is saved in the trail journal.")
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim("/retry to try the turn again · /model <name> to switch oxen first"),
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim(&format!(
            "later: oxen-harness --continue (or --resume {}), then /retry",
            agent.session_id()
        )),
    ));
    lines
}

/// Whether the sticky-bottom live composer should drive this turn: only for an
/// interactive, animating TTY. Pipes, tests, `NO_COLOR`, and `TERM=dumb` fall
/// back to the classic blocking prompt with unchanged behavior.
pub(crate) fn live_enabled(ui: &Ui) -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && ui.animates()
}

/// Run one turn (a prompt or a `/retry` continuation), then auto-drain any
/// stacked messages in order. On a live TTY this hands off to the sticky
/// composer (which lets the user keep stacking while turns run); otherwise it
/// runs the classic prompt and drains after.
/// Returns `Ok(true)` when the session should end.
pub(crate) async fn run_turn_and_drain(
    agent: &mut Agent,
    request: TurnRequest,
    ui: &Ui,
    queue: &mut MessageQueue,
    carryover: &mut String,
) -> Result<bool> {
    if live_enabled(ui) {
        // The live composer hands back any half-typed next message so the idle
        // prompt can keep it instead of wiping it when the turn ends.
        let (exit, draft) = live::run_prompt(agent, request, ui, queue).await?;
        *carryover = draft;
        return Ok(exit);
    }
    if run_prompt(agent, &request, ui).await? {
        return Ok(true);
    }
    while !queue.is_empty() {
        let next = queue.pop_front().expect("queue is non-empty");
        println!(
            "  {} {}",
            ui.brown("▶ rolling the wagon:"),
            ui.cream(&truncate(&next, 80)),
        );
        if run_prompt(agent, &TurnRequest::Prompt(next), ui).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Run one turn, racing it against Ctrl-C. Returns `Ok(true)` if the user
/// interrupted the stream (the caller should then end the session).
async fn run_prompt(agent: &mut Agent, request: &TurnRequest, ui: &Ui) -> Result<bool> {
    let (text, attachments) = match request {
        TurnRequest::Prompt(prompt) => {
            let (text, attachments, warnings) = attach::extract_attachments(prompt);
            for w in &warnings {
                println!("  {} {}", ui.red("⚠"), ui.dim(w));
            }
            if !attachments.is_empty() {
                let names: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
                println!(
                    "  {} {}",
                    ui.green("📎 attached:"),
                    ui.cream(&names.join(", "))
                );
            }
            (text, attachments)
        }
        // A retry re-drives the transcript as-is; there is no new message.
        TurnRequest::Continue => (String::new(), Vec::new()),
    };

    let renderer = Rc::new(RefCell::new(TurnRenderer::new(ui.clone())));
    renderer.borrow_mut().begin_thinking();

    let cb = renderer.clone();
    let mut on_event = move |event: &harness_agent::AgentEvent| cb.borrow_mut().on_event(event);
    let is_continue = matches!(request, TurnRequest::Continue);
    let result = tokio::select! {
        // Cancel the in-flight turn on Ctrl-C and signal an exit.
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            return Ok(true);
        }
        result = async {
            if is_continue {
                agent.continue_turn(&mut on_event).await
            } else {
                agent.run_turn_with_attachments(text, attachments, &mut on_event).await
            }
        } => result,
    };
    renderer.borrow_mut().finish();
    let needs_brave_key = renderer.borrow().needs_brave_key();

    match result {
        Ok(_) => {
            print_context_usage(agent, ui);
            // Offer to set up web search if the model tried it without a key.
            if needs_brave_key {
                brave::prompt_after_failed_search(ui);
            }
            Ok(false)
        }
        Err(e) => {
            println!();
            for line in turn_failure_lines(agent, ui, &e) {
                println!("{line}");
            }
            Ok(false)
        }
    }
}

/// A subtle trailer showing how full the model's context window is, set apart
/// from the turn's output by a blank line.
fn print_context_usage(agent: &Agent, ui: &Ui) {
    println!();
    println!("{}", context_usage_line(agent, ui));
}

/// The context-usage trailer as a (themed, indented) line — the current model
/// alongside how full its context window is. Shared by the classic prompt and
/// the live composer (which pins it just above the input divider).
pub(crate) fn context_usage_line(agent: &Agent, ui: &Ui) -> String {
    let used = agent.context_tokens();
    let window = agent.context_window();
    let pct = (used * 100).checked_div(window).map_or(0, |p| p.min(100));
    format!(
        "  {} {}",
        ui.dim(&format!(
            "🧭 context {} / {} tokens ({pct}%) ·",
            human_tokens(used),
            human_tokens(window),
        )),
        ui.accent(agent.model()),
    )
}

/// Human-friendly token count: `980`, `12.3k`, `1.2M`.
// Token counts render identically everywhere; the shared formatter lives in
// harness-core and is re-exported here for the CLI's meters and lanes.
pub(crate) use harness_core::fmt::human_tokens;

#[cfg(test)]
mod tests {
    use super::{ends_mid_turn, retry_notice};
    use harness_llm::ChatMessage;

    #[test]
    fn ends_mid_turn_flags_dangling_user_and_tool_messages() {
        // Ends on a user message: the reply never arrived → retryable.
        let dangling_user = vec![ChatMessage::system("s"), ChatMessage::user("hi")];
        assert!(ends_mid_turn(&dangling_user));

        // Ends on a tool result the model never reacted to → retryable.
        let dangling_tool = vec![
            ChatMessage::user("hi"),
            ChatMessage::tool_result("t1", "output".to_string()),
        ];
        assert!(ends_mid_turn(&dangling_tool));

        // A settled conversation (assistant spoke last) has nothing to retry.
        let settled = vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")];
        assert!(!ends_mid_turn(&settled));
        assert!(!ends_mid_turn(&[]));
    }

    #[test]
    fn retry_notice_reports_the_upcoming_attempt_and_wait() {
        let notice = retry_notice(1, 4, 2000, "Oxen API error (502): provider error");
        assert!(notice.contains("Oxen API error (502)"));
        assert!(notice.contains("retrying in 2s"));
        assert!(notice.contains("attempt 2 of 4"));
    }
}
