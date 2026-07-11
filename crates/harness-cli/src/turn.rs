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

/// Whether a failed turn is worth pre-filling `/retry` for: the transcript
/// stops mid-turn (so there's a dangling turn to re-drive) and the failure
/// isn't an auth one (where `/auth` is the fix, not a retry). Both terminals
/// use this to seed the next prompt with `/retry` so a bare ⏎ tries again.
pub(crate) fn seed_retry(
    messages: &[harness_llm::ChatMessage],
    err: &harness_agent::AgentError,
) -> bool {
    ends_mid_turn(messages) && !commands::auth::is_auth_error(&err.to_string())
}

/// The failure report for a turn that died even after retries: what happened,
/// then how to pick the conversation back up — now (`/retry`, `/model`) or
/// later (`--continue` / `--resume`). Auth failures get the `/auth` hint
/// instead of the generic recovery lines; `retry_seeded` says the prompt was
/// pre-filled with `/retry`, so the hint points at ⏎ instead. Shared by the
/// classic prompt and the live composer so both terminals explain the same
/// way out.
pub(crate) fn turn_failure_lines(
    agent: &Agent,
    ui: &Ui,
    err: &harness_agent::AgentError,
    retry_seeded: bool,
) -> Vec<String> {
    let mut lines = vec![format!("  {}", ui.red(&ui.death()))];
    // Retries exhausted is the "endpoint is down" case: break the report into
    // what failed, the last error, and where — a headline the eye can't skip,
    // instead of one long dim line.
    if let harness_agent::AgentError::RetriesExhausted {
        attempts,
        model,
        endpoint,
        source,
    } = err
    {
        lines.push(format!(
            "  {}",
            ui.red(&format!(
                "The model endpoint failed {attempts} times in a row — the turn did not finish."
            )),
        ));
        lines.push(format!("  {}", ui.cream(&format!("Last error: {source}"))));
        lines.push(format!("  {}", ui.dim(&format!("({model} at {endpoint})"))));
    } else {
        lines.push(format!(
            "  {}",
            ui.dim(&format!("The trail guide says: {err}"))
        ));
    }
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
        ui.dim(if retry_seeded {
            "/retry is ready below — press ⏎ to try again · /model <name> to switch oxen first"
        } else {
            "/retry to try the turn again · /model <name> to switch oxen first"
        }),
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim(&format!(
            "later: oxen-harness --continue (or --resume {}), then /retry",
            agent.session_id()
        )),
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim("full error details: ~/.oxen-harness/errors.jsonl"),
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
/// Returns `Ok(true)` when the session should end (Ctrl-D). A Ctrl-C only
/// cancels the running turn and its drain — the caller returns to the prompt.
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
    if matches!(
        run_prompt(agent, &request, ui, carryover).await?,
        PromptOutcome::Interrupted
    ) {
        return Ok(false);
    }
    while !queue.is_empty() {
        let next = queue.pop_front().expect("queue is non-empty");
        println!(
            "  {} {}",
            ui.brown("▶ rolling the wagon:"),
            ui.cream(&truncate(&next, 80)),
        );
        if matches!(
            run_prompt(agent, &TurnRequest::Prompt(next), ui, carryover).await?,
            PromptOutcome::Interrupted
        ) {
            return Ok(false);
        }
    }
    Ok(false)
}

/// How one classic-prompt turn ended.
enum PromptOutcome {
    /// The turn ran to completion (success or a reported failure).
    Ran,
    /// Ctrl-C cancelled the stream — stop draining, but keep the session.
    Interrupted,
}

/// Run one turn, racing it against Ctrl-C. A Ctrl-C cancels the in-flight
/// turn and returns [`PromptOutcome::Interrupted`] so the caller stops
/// draining the queue — the session itself continues at the prompt.
///
/// `carryover` seeds the next idle prompt: a retryable failure fills it with
/// `/retry` so a bare ⏎ re-drives the dangling turn; a finished turn clears
/// any stale seed (in the classic prompt this string is ours alone — there is
/// no mid-turn composer that could hold the user's typing).
async fn run_prompt(
    agent: &mut Agent,
    request: &TurnRequest,
    ui: &Ui,
    carryover: &mut String,
) -> Result<PromptOutcome> {
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
        // Ctrl-C cancels the in-flight turn; the session continues.
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            println!();
            println!(
                "  {} {}",
                ui.red("⚠ interrupted"),
                ui.dim("— the oxen pull up short"),
            );
            println!(
                "  {}",
                ui.dim(if ends_mid_turn(agent.messages()) {
                    "every step so far is saved · /retry continues this turn, or just give new directions"
                } else {
                    "every step so far is saved · give new directions whenever you're ready"
                }),
            );
            return Ok(PromptOutcome::Interrupted);
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
            carryover.clear();
            print_context_usage(agent, ui);
            // Offer to set up web search if the model tried it without a key.
            if needs_brave_key {
                brave::prompt_after_failed_search(ui);
            }
            Ok(PromptOutcome::Ran)
        }
        Err(e) => {
            println!();
            let seeded = seed_retry(agent.messages(), &e);
            for line in turn_failure_lines(agent, ui, &e, seeded) {
                println!("{line}");
            }
            if seeded {
                *carryover = "/retry".to_string();
            }
            Ok(PromptOutcome::Ran)
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
    use super::{ends_mid_turn, retry_notice, seed_retry};
    use harness_agent::AgentError;
    use harness_llm::{ChatMessage, LlmError};

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
    fn seed_retry_only_for_dangling_retryable_failures() {
        let dangling = vec![ChatMessage::system("s"), ChatMessage::user("hi")];
        let settled = vec![ChatMessage::user("hi"), ChatMessage::assistant("done")];
        let exhausted = AgentError::RetriesExhausted {
            attempts: 4,
            model: "claude-opus-4-8".into(),
            endpoint: "https://hub.oxen.ai/api/ai".into(),
            source: LlmError::Api {
                status: 502,
                message: "The model provider returned an error.".into(),
            },
        };
        let auth = AgentError::Llm(LlmError::Api {
            status: 401,
            message: "Invalid API key".into(),
        });

        // A dangling turn that died on a provider error → pre-fill /retry.
        assert!(seed_retry(&dangling, &exhausted));
        // A settled conversation has nothing to re-drive.
        assert!(!seed_retry(&settled, &exhausted));
        // Auth failures need /auth first, not a retry.
        assert!(!seed_retry(&dangling, &auth));
    }

    #[test]
    fn retry_notice_reports_the_upcoming_attempt_and_wait() {
        let notice = retry_notice(1, 4, 2000, "Oxen API error (502): provider error");
        assert!(notice.contains("Oxen API error (502)"));
        assert!(notice.contains("retrying in 2s"));
        assert!(notice.contains("attempt 2 of 4"));
    }
}
