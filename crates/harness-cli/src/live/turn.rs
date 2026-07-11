//! Turn orchestration for the live composer: own the terminal, drive the
//! agent's turn future and the input stream side by side, and drain the queue.
//!
//! [`run_prompt`] owns a whole prompt-then-drain sequence under one raw-mode
//! terminal; [`read_idle`] reads a single submission at the idle prompt with
//! the same pinned composer. Both hand their keystrokes to
//! [`Live::handle_key`](super::Live) and let the returned [`KeyAction`] drive
//! the queue, so idle and mid-turn input behave identically.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::Event;
use harness_agent::{Agent, AgentError, AgentEvent};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::queue::MessageQueue;
use crate::render::truncate;
use crate::theme::Ui;

use super::composer::{Composer, History};
use super::keys::KeyAction;
use super::terminal::{spawn_input, LiveTerminal};
use super::text::composer_prompt;
use super::Live;

/// Run `first` (a prompt, or a `/retry` continuation of the transcript's
/// dangling turn), then drain any queued prompts in order, all under a single
/// owned live terminal so the composer stays pinned across the whole sequence.
///
/// Returns `(end_session, draft)`: `end_session` is true when the user asked to
/// quit (Ctrl-D on an empty composer); Ctrl-C only cancels the in-flight turn
/// and hands back to the idle prompt. `draft` is whatever unsent text was left
/// in the composer, so the caller can seed the idle prompt with it (a
/// half-typed next message isn't lost when the turn ends or is interrupted).
pub(crate) async fn run_prompt(
    agent: &mut Agent,
    first: crate::turn::TurnRequest,
    ui: &Ui,
    queue: &mut MessageQueue,
) -> Result<(bool, String)> {
    let term = LiveTerminal::new()?;
    let (rows, cols) = (term.rows, term.cols);
    // While the composer owns the terminal, a `spawn_agents` fleet must paint
    // through its pinned area (not a painter thread of its own). The guard
    // clears the flag on every exit from this function — including the early
    // returns below — so a cooked-mode fleet is never stranded un-painted.
    let fleet_hub = crate::fleet_ui::FleetHub::global();
    let _live = fleet_hub.mark_live();

    // The input thread only ever reads key/resize events and forwards them; it
    // never writes to the terminal. `stop` ends it; `paused` makes it yield the
    // event stream to an interactive tool (the picker) mid-turn.
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (mut rx, input) = spawn_input(&stop, &paused);

    let state = Rc::new(RefCell::new(Live::new(ui.clone(), cols, rows)));
    // Show the meters in their pinned slots from the start of the turn.
    {
        let mut s = state.borrow_mut();
        s.status_line = Some(crate::turn::context_usage_line(agent, ui));
        s.compression_line = crate::commands::compression::status_line(agent, ui);
    }

    let mut next = Some(first);
    let mut exit = false;
    while let Some(request) = next.take() {
        match run_one_turn(agent, &request, &state, &mut rx, queue, &paused).await {
            TurnOutcome::Interrupted => {
                // Ctrl-C stops the agentic loop, not the session: cancel the
                // in-flight turn (and any queued drain), report how to pick it
                // back up, and fall through to the idle prompt.
                let mut s = state.borrow_mut();
                let ui = s.ui.clone();
                s.print_line(&format!(
                    "  {} {}",
                    ui.red("⚠ interrupted"),
                    ui.dim("— the oxen pull up short"),
                ));
                s.print_line(&format!(
                    "  {}",
                    ui.dim(if crate::turn::ends_mid_turn(agent.messages()) {
                        "every step so far is saved · /retry continues this turn, or just give new directions"
                    } else {
                        "every step so far is saved · give new directions whenever you're ready"
                    }),
                ));
                s.render();
                break;
            }
            TurnOutcome::Done(result) => {
                {
                    let mut s = state.borrow_mut();
                    if let Err(e) = &result {
                        // Pre-fill the composer with /retry when there's a
                        // dangling turn to re-drive and nothing else pending,
                        // so a bare ⏎ picks the trail back up.
                        let seed = crate::turn::seed_retry(agent.messages(), e)
                            && queue.is_empty()
                            && s.composer_draft().is_empty();
                        for line in crate::turn::turn_failure_lines(agent, ui, e, seed) {
                            s.print_line(&line);
                        }
                        if seed {
                            s.composer.set_text("/retry");
                        }
                    }
                    // Refresh the pinned meters (they sit above the divider,
                    // not in the scrollback) with the turn's totals.
                    s.status_line = Some(crate::turn::context_usage_line(agent, ui));
                    s.compression_line = crate::commands::compression::status_line(agent, ui);
                    s.render();
                }
                // Auto-drain: send the next stacked message (more may still be
                // typed while it runs — they just keep stacking onto the queue).
                if !queue.is_empty() {
                    let msg = queue.pop_front().expect("queue is non-empty");
                    let mut s = state.borrow_mut();
                    s.sync_queue(queue.items());
                    s.print_line(&format!(
                        "  {} {}",
                        ui.brown("▶ rolling the wagon:"),
                        ui.cream(&truncate(&msg, 80)),
                    ));
                    s.render();
                    next = Some(crate::turn::TurnRequest::Prompt(msg));
                }
            }
            TurnOutcome::Exit => {
                exit = true;
                break;
            }
        }
    }

    // Stop the input thread (its poll timeout lets it exit promptly) and join it
    // before restoring the terminal.
    stop.store(true, Ordering::Relaxed);
    let _ = input.join();
    let needs_brave_key = state.borrow().needs_brave_key;
    // Capture any half-typed message so the idle prompt can keep it (unless the
    // session is ending, where it's moot).
    let draft = if exit {
        String::new()
    } else {
        state.borrow().composer_draft()
    };
    drop(state);
    drop(term);
    drop(_live); // clear the live flag before any cooked-mode prompt below

    // Now that the alt-screen/raw mode is torn down, offer to set up web search
    // if the agent tried it without a Brave key during the session.
    if needs_brave_key {
        crate::brave::prompt_after_failed_search(ui);
    }
    Ok((exit, draft))
}

/// The result of reading one submission at the idle prompt.
pub(crate) enum Idle {
    /// The user submitted this (a prompt to run or a `/command`).
    Submit(String),
    /// Ctrl-D on an empty box, or a confirmed double Ctrl-C — end the session.
    Exit,
}

/// What one idle-prompt Ctrl-C should do, staged Claude-Code-style: first
/// clear whatever is being typed, then ask for confirmation, and only a
/// confirmed second press actually leaves.
#[derive(Debug, PartialEq, Eq)]
enum InterruptStage {
    /// There's a draft (or an in-progress queue edit) — wipe it, stay.
    ClearDraft,
    /// Nothing to clear and not yet armed — warn that another Ctrl-C exits.
    Arm,
    /// Already armed — leave the session.
    Exit,
}

fn interrupt_stage(has_draft: bool, armed: bool) -> InterruptStage {
    if has_draft {
        InterruptStage::ClearDraft
    } else if armed {
        InterruptStage::Exit
    } else {
        InterruptStage::Arm
    }
}

/// Read one submission at the idle prompt using the pinned composer, then
/// tear the terminal down so the caller can run a turn or a command in cooked
/// mode. `seed` pre-fills the input (e.g. a draft carried over from a turn);
/// `history` is loaded for Up/Down recall and updated with the submission;
/// `status` is the context-usage line pinned just above the divider, and
/// `compression` the savings line pinned just above that.
///
/// Returns [`Idle::Submit`] with the trimmed text, or [`Idle::Exit`] to quit.
pub(crate) async fn read_idle(
    ui: &Ui,
    queue: &mut MessageQueue,
    history: &mut Vec<String>,
    seed: &str,
    status: Option<String>,
    compression: Option<String>,
) -> Result<Idle> {
    let term = LiveTerminal::new()?;
    let (rows, cols) = (term.rows, term.cols);
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (mut rx, input) = spawn_input(&stop, &paused);

    let state = Rc::new(RefCell::new(Live::new(ui.clone(), cols, rows)));
    {
        let mut s = state.borrow_mut();
        s.history = History::with_entries(history.clone());
        if !seed.is_empty() {
            s.composer = Composer::seeded(seed);
        }
        s.status_line = status;
        s.compression_line = compression;
        s.sync_queue(queue.items());
        s.render();
    }

    // Whether the next Ctrl-C exits (armed by a Ctrl-C on an empty composer;
    // any other key disarms it).
    let mut exit_armed = false;
    let result = loop {
        match rx.recv().await {
            Some(Event::Key(key)) => {
                let action = state.borrow_mut().handle_key(key, queue.len());
                if !matches!(action, KeyAction::Interrupt) {
                    exit_armed = false;
                }
                match action {
                    KeyAction::None => {}
                    KeyAction::Redraw => state.borrow_mut().render(),
                    KeyAction::Submit(line) => {
                        // At idle, Enter sends (vs. queueing during a turn).
                        let trimmed = line.trim().to_string();
                        if trimmed.is_empty() {
                            state.borrow_mut().render();
                        } else {
                            break Idle::Submit(trimmed);
                        }
                    }
                    KeyAction::BeginEdit => {
                        let mut s = state.borrow_mut();
                        if let Some(i) = s.focused_item() {
                            if let Some(text) = queue.items().get(i) {
                                s.begin_edit(text);
                            }
                        }
                        s.render();
                    }
                    KeyAction::SaveEdit => {
                        let mut s = state.borrow_mut();
                        if let Some((idx, text)) = s.take_edit() {
                            let _ = queue.edit(idx + 1, text);
                            s.sync_queue(queue.items());
                        }
                        s.render();
                    }
                    KeyAction::CancelEdit => {
                        let mut s = state.borrow_mut();
                        s.cancel_edit();
                        s.render();
                    }
                    KeyAction::DeleteFocused => {
                        let mut s = state.borrow_mut();
                        if let Some(i) = s.focused_item() {
                            let _ = queue.remove(i + 1);
                        }
                        s.sync_queue(queue.items());
                        s.render();
                    }
                    KeyAction::Exit => break Idle::Exit,
                    KeyAction::Interrupt => {
                        // Staged Ctrl-C: clear the draft first, then confirm,
                        // then exit — never a surprise quit mid-thought.
                        let mut s = state.borrow_mut();
                        let has_draft = !s.composer_draft().is_empty();
                        match interrupt_stage(has_draft, exit_armed) {
                            InterruptStage::ClearDraft => {
                                s.cancel_edit();
                                s.composer.set_text("");
                                s.render();
                            }
                            InterruptStage::Arm => {
                                exit_armed = true;
                                let ui = s.ui.clone();
                                s.print_line(&format!(
                                    "  {} {}",
                                    ui.red("⚠"),
                                    ui.dim(
                                        "press ctrl-c again to leave the trail — \
                                         any other key keeps riding"
                                    ),
                                ));
                                s.render();
                            }
                            InterruptStage::Exit => break Idle::Exit,
                        }
                    }
                }
            }
            Some(Event::Resize(c, r)) => {
                state.borrow_mut().handle_resize(c, r, queue.items());
            }
            Some(Event::Paste(text)) => {
                exit_armed = false;
                let mut s = state.borrow_mut();
                s.insert_paste(&text);
                s.render();
            }
            Some(_) => {}
            None => break Idle::Exit,
        }
    };

    *history = state.borrow().history.entries().to_vec();
    stop.store(true, Ordering::Relaxed);
    let _ = input.join();
    drop(state);
    drop(term);

    // Echo the submission into the scrollback (cooked mode) so it stays above
    // whatever output the turn/command prints next — mirroring how a typed
    // prompt used to remain on screen.
    if let Idle::Submit(text) = &result {
        let (_, styled) = composer_prompt(ui, queue.len());
        println!("{styled}{}", ui.cream(text));
    }
    Ok(result)
}

/// Extract a short "target" from a tool's JSON `arguments` to show beside the
/// spinner verb — the file being read/written, the shell command, the search
/// query, etc. — so the activity line says *what* it's working on. Returns
/// `None` when there's nothing useful (or the args don't parse), in which case
/// the spinner just shows the verb + timer.
pub(crate) fn tool_target(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    // Try the fields most tools use, in rough priority order.
    let raw = ["path", "file", "command", "query", "pattern", "url"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .filter(|s| !s.is_empty())?;
    // Keep it to a single line and bounded so the indicator never wraps.
    let one_line = raw.split(['\n', '\r']).next().unwrap_or(raw).trim();
    Some(truncate(one_line, 60))
}

/// What happened to a single turn in the live loop.
/// Whether a mid-turn submission can stack onto the message queue: only plain
/// prompts — the queue drains as *prompts for the model*, so a recognized
/// `/command` would reach the LLM as literal chat text instead of running.
fn stackable(text: &str) -> bool {
    matches!(
        crate::repl::parse_command(text),
        crate::repl::Command::Prompt(_)
    )
}

enum TurnOutcome {
    /// The turn finished on its own (success or agent error).
    Done(Result<String, AgentError>),
    /// Ctrl-C — cancel this turn and its queued drain; the session continues
    /// at the idle prompt.
    Interrupted,
    /// Ctrl-D on an empty composer — end the session.
    Exit,
}

/// Drive one `run_turn` to completion while servicing the composer: keystrokes
/// stack onto `queue`, the spinner advances on a tick, and every event redraws
/// the bottom line.
async fn run_one_turn(
    agent: &mut Agent,
    request: &crate::turn::TurnRequest,
    state: &Rc<RefCell<Live>>,
    rx: &mut UnboundedReceiver<Event>,
    queue: &mut MessageQueue,
    paused: &Arc<AtomicBool>,
) -> TurnOutcome {
    // Pull any dropped files out of the line and announce them in the scroll
    // region before the turn starts. A `/retry` continuation carries no new
    // message, so there is nothing to extract or announce.
    let (text, attachments) = match request {
        crate::turn::TurnRequest::Prompt(prompt) => {
            let (text, attachments, warnings) = crate::attach::extract_attachments(prompt);
            let mut st = state.borrow_mut();
            let ui = st.ui.clone();
            for w in &warnings {
                st.print_line(&format!("  {} {}", ui.red("⚠"), ui.dim(w)));
            }
            if !attachments.is_empty() {
                let names: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
                st.print_line(&format!(
                    "  {} {}",
                    ui.green("📎 attached:"),
                    ui.cream(&names.join(", "))
                ));
            }
            (text, attachments)
        }
        crate::turn::TurnRequest::Continue => (String::new(), Vec::new()),
    };

    state.borrow_mut().begin_turn(queue.items());

    let cb = state.clone();
    let cb_paused = paused.clone();
    let on_event = move |event: &AgentEvent| {
        cb.borrow_mut().on_event(event, &cb_paused);
    };
    let turn: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, AgentError>> + '_>,
    > = match request {
        crate::turn::TurnRequest::Prompt(_) => {
            Box::pin(agent.run_turn_with_attachments(text, attachments, on_event))
        }
        crate::turn::TurnRequest::Continue => Box::pin(agent.continue_turn(on_event)),
    };
    tokio::pin!(turn);

    let mut ticker = tokio::time::interval(Duration::from_millis(110));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = &mut turn => {
                state.borrow_mut().finish();
                return TurnOutcome::Done(result);
            }
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(Event::Key(key)) => {
                        let action = state.borrow_mut().handle_key(key, queue.len());
                        match action {
                            KeyAction::None => {}
                            KeyAction::Redraw => state.borrow_mut().render(),
                            KeyAction::Submit(line) => {
                                let trimmed = line.trim();
                                let mut s = state.borrow_mut();
                                if trimmed.is_empty() {
                                    // Nothing to stack.
                                } else if stackable(trimmed) {
                                    queue.add(trimmed);
                                } else {
                                    // A /command can't stack — the queue drains
                                    // as prompts for the model, which would
                                    // receive "/model …" as chat text. Keep it
                                    // in the composer to run once the turn ends.
                                    let ui = s.ui.clone();
                                    s.print_line(&format!(
                                        "  {} {}",
                                        ui.brown("⛺"),
                                        ui.dim(
                                            "commands don't stack in the wagon — \
                                             kept in the composer to run after this turn"
                                        ),
                                    ));
                                    s.composer.set_text(trimmed);
                                }
                                s.sync_queue(queue.items());
                                s.render();
                            }
                            KeyAction::BeginEdit => {
                                let mut s = state.borrow_mut();
                                if let Some(i) = s.focused_item() {
                                    if let Some(text) = queue.items().get(i) {
                                        s.begin_edit(text);
                                    }
                                }
                                s.render();
                            }
                            KeyAction::SaveEdit => {
                                let mut s = state.borrow_mut();
                                if let Some((idx, text)) = s.take_edit() {
                                    let _ = queue.edit(idx + 1, text);
                                    s.sync_queue(queue.items());
                                }
                                s.render();
                            }
                            KeyAction::CancelEdit => {
                                let mut s = state.borrow_mut();
                                s.cancel_edit();
                                s.render();
                            }
                            KeyAction::DeleteFocused => {
                                let mut s = state.borrow_mut();
                                if let Some(i) = s.focused_item() {
                                    let _ = queue.remove(i + 1);
                                }
                                s.sync_queue(queue.items());
                                s.render();
                            }
                            KeyAction::Interrupt => {
                                state.borrow_mut().finish();
                                return TurnOutcome::Interrupted;
                            }
                            KeyAction::Exit => {
                                state.borrow_mut().finish();
                                return TurnOutcome::Exit;
                            }
                        }
                    }
                    Some(Event::Resize(cols, rows)) => {
                        state.borrow_mut().handle_resize(cols, rows, queue.items());
                    }
                    Some(Event::Paste(text)) => {
                        let mut s = state.borrow_mut();
                        s.insert_paste(&text);
                        s.render();
                    }
                    Some(_) => {}
                    None => {}
                }
            }
            _ = ticker.tick() => {
                let mut s = state.borrow_mut();
                s.tick_spinner();
                // A running fleet animates in the pinned area on the same tick.
                if s.tick_fleet() {
                    s.render();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stackable;

    #[test]
    fn interrupt_stages_clear_then_arm_then_exit() {
        use super::{interrupt_stage, InterruptStage};
        // Something typed: the first Ctrl-C only clears it (armed or not).
        assert_eq!(interrupt_stage(true, false), InterruptStage::ClearDraft);
        assert_eq!(interrupt_stage(true, true), InterruptStage::ClearDraft);
        // Nothing typed: warn first, exit only on the confirmed second press.
        assert_eq!(interrupt_stage(false, false), InterruptStage::Arm);
        assert_eq!(interrupt_stage(false, true), InterruptStage::Exit);
    }

    #[test]
    fn only_plain_prompts_stack_onto_the_queue() {
        // Prompts (including slash-prefixed text no command recognizes) stack.
        assert!(stackable("fix the failing test"));
        assert!(stackable("/unknown thing"));
        // Recognized commands don't — they'd reach the model as chat text.
        assert!(!stackable("/model claude-sonnet-4-6"));
        assert!(!stackable("/retry"));
        assert!(!stackable("/q"));
    }
}
