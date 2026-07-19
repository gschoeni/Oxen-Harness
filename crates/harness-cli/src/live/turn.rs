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

use crate::interrupt::{arm_notice, interrupted_lines, CtrlC, ExitGuard};

use super::composer::{Composer, History};
use super::dispatch::{apply_action, Residual};
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
        // Remember the model + window so mid-turn `Usage` events can rebuild the
        // context trailer live (they carry token counts, not these fixed facts).
        s.model = agent.model().to_string();
        s.context_window = agent.context_window();
        s.status_lines = crate::turn::context_usage_lines(agent, ui);
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
                for line in interrupted_lines(&ui, crate::turn::ends_mid_turn(agent.messages())) {
                    s.print_line(&line);
                }
                s.render();
                break;
            }
            TurnOutcome::Done(result) => {
                // Learn this model's rate (once) so the refreshed trailer can
                // show the session's running cost. Cheap when already cached.
                crate::pricing::warm_for(agent.model()).await;
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
                    s.status_lines = crate::turn::context_usage_lines(agent, ui);
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
    // Erase the session's chrome (meters, divider, composer) on the way out —
    // only conversation belongs in the scrollback. The next cooked-mode print
    // continues right below the last output line.
    let region_bottom = state.borrow().region_bottom;
    drop(state);
    term.restore(region_bottom);
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

/// Read one submission at the idle prompt using the pinned composer, then
/// tear the terminal down so the caller can run a turn or a command in cooked
/// mode. `seed` pre-fills the input (e.g. a draft carried over from a turn);
/// `history` is loaded for Up/Down recall and updated with the submission;
/// `status` is the context-usage trailer (its two lines) pinned just above the
/// divider, and `compression` the savings line pinned just above that.
///
/// Returns [`Idle::Submit`] with the trimmed text, or [`Idle::Exit`] to quit.
pub(crate) async fn read_idle(
    ui: &Ui,
    queue: &mut MessageQueue,
    history: &mut Vec<String>,
    seed: &str,
    status: Vec<String>,
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
        s.status_lines = status;
        s.compression_line = compression;
        s.sync_queue(queue.items());
        s.render();
    }

    // Staged Ctrl-C (armed by a Ctrl-C on an empty composer; any other key
    // disarms it) — the same guard the classic REPL uses.
    let mut guard = ExitGuard::default();
    let result = loop {
        // The idle loop has no ticker; when a key-event-burst media check is
        // pending, wake at its settle deadline so a dropped path with no
        // trailing delimiter still collapses to a chip. (Bound first: a
        // scrutinee temporary would hold the borrow across the whole match.)
        let media_due = state.borrow().media_check_due();
        let event = match media_due {
            Some(due) => tokio::select! {
                ev = rx.recv() => ev,
                _ = tokio::time::sleep_until(due.into()) => {
                    let mut s = state.borrow_mut();
                    if s.tick_media_check() {
                        s.request_paint();
                    }
                    s.flush_paint();
                    continue;
                }
            },
            None => rx.recv().await,
        };
        match event {
            Some(Event::Key(key)) => {
                let action = state.borrow_mut().handle_key(key, queue.len());
                if !matches!(action, KeyAction::Interrupt) {
                    guard.disarm();
                }
                let residual = apply_action(&mut state.borrow_mut(), queue, action);
                match residual {
                    None => {}
                    Some(Residual::Submit(line)) => {
                        // At idle, Enter sends (vs. queueing during a turn).
                        let trimmed = line.trim().to_string();
                        if trimmed.is_empty() {
                            state.borrow_mut().request_paint();
                        } else {
                            break Idle::Submit(trimmed);
                        }
                    }
                    Some(Residual::Exit) => break Idle::Exit,
                    Some(Residual::Interrupt) => {
                        // Staged Ctrl-C: clear the draft first, then confirm,
                        // then exit — never a surprise quit mid-thought.
                        let mut s = state.borrow_mut();
                        let has_draft = !s.composer_draft().is_empty();
                        match guard.on_ctrl_c(has_draft) {
                            CtrlC::ClearDraft => {
                                s.cancel_edit();
                                s.composer.set_text("");
                                s.request_paint();
                            }
                            CtrlC::Arm => {
                                let ui = s.ui.clone();
                                s.print_line(&arm_notice(&ui));
                            }
                            CtrlC::Exit => break Idle::Exit,
                        }
                    }
                }
                state.borrow_mut().flush_paint();
            }
            Some(Event::Resize(c, r)) => {
                state.borrow_mut().handle_resize(c, r, queue.items());
            }
            Some(Event::Paste(text)) => {
                guard.disarm();
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
    // Erase the idle chrome (meters, divider, completion hint, composer) on
    // the way out — only conversation belongs in the scrollback, and the
    // echoed submission below prints right where the conversation left off.
    let region_bottom = state.borrow().region_bottom;
    drop(state);
    term.restore(region_bottom);

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
/// A message pushed in the instant after the turn's final drain (or during a
/// cancelled turn) would otherwise vanish — recover it onto the queue so it
/// still runs, as the next prompt.
fn recover_interjections(
    interject: &harness_agent::Interjections,
    queue: &mut crate::queue::MessageQueue,
) {
    for msg in interject.take_all() {
        queue.add(msg);
    }
}

/// Whether a mid-turn submission can be sent to the model as chat (steered
/// into a running turn, or queued): only plain prompts — a recognized
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

    // The steering channel into the running turn: messages pushed here are
    // drained at the loop's safe points, so the model sees them mid-work.
    // Cloned before the turn future takes `agent` (like the cancel token).
    let interject = agent.interjections();

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
                recover_interjections(&interject, queue);
                return TurnOutcome::Done(result);
            }
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(Event::Key(key)) => {
                        let action = state.borrow_mut().handle_key(key, queue.len());
                        let residual = apply_action(&mut state.borrow_mut(), queue, action);
                        match residual {
                            None => {}
                            Some(Residual::Submit(line)) => {
                                let trimmed = line.trim();
                                let mut s = state.borrow_mut();
                                if trimmed.is_empty() {
                                    // Nothing to send.
                                } else if stackable(trimmed) {
                                    // Steer the running turn: the message is
                                    // delivered into it at the next safe point
                                    // (not queued for after). Stack follow-up
                                    // prompts for later with /queue instead.
                                    interject.push(trimmed);
                                    let ui = s.ui.clone();
                                    s.print_line(&format!(
                                        "  {} {}",
                                        ui.brown("🗣 steering:"),
                                        ui.cream(&truncate(trimmed, 80)),
                                    ));
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
                                s.request_paint();
                            }
                            Some(Residual::Interrupt) => {
                                // Mid-turn, Ctrl-C cancels the running turn —
                                // that *is* its clear-first stage; the idle
                                // prompt then owns the staged exit.
                                state.borrow_mut().finish();
                                recover_interjections(&interject, queue);
                                return TurnOutcome::Interrupted;
                            }
                            Some(Residual::Exit) => {
                                state.borrow_mut().finish();
                                return TurnOutcome::Exit;
                            }
                        }
                        state.borrow_mut().flush_paint();
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
                    s.request_paint();
                }
                // A settled key-event-burst drop collapses to a media chip here.
                if s.tick_media_check() {
                    s.request_paint();
                }
                s.flush_paint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stackable;

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
