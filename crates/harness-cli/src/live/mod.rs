//! The live, bottom-pinned input box for the interactive REPL.
//!
//! On an interactive terminal this replaces `rustyline`: the input is a
//! frameless prompt area pinned to the bottom (no border characters, so
//! terminal selections copy clean), with the themed prompt, **multi-line**
//! editing (Alt/Shift+Enter or Ctrl-J adds a line), **history** recall (Up/Down at
//! a line edge), and the stacked [`MessageQueue`] above it. The same box is used
//! whether idle ([`read_idle`]) or mid-turn ([`run_prompt`]) — while the agent
//! streams you can keep typing, and Enter stacks a follow-up onto the queue that
//! drains when the turn finishes.
//!
//! How it stays out of the output's way: we put the terminal in raw mode, pin the
//! box to the bottom rows, keep a blank spacer + a faint divider just above it
//! (so output never butts against the input — see [`SPACER_ROWS`]/[`DIVIDER_ROWS`]),
//! and set a DECSTBM scroll region over the rows *above* that. All turn output
//! (streamed Markdown, tool lines, the spinner) is written into that region —
//! where it scrolls naturally — through a small adapter that turns `\n` into
//! `\r\n` (mandatory in raw mode). The box is repainted after every output event
//! and keystroke, bracketed by save/restore-cursor so the output is never
//! disturbed; the box grows/shrinks with the lines typed, re-carving the region.
//!
//! Entered only for an interactive TTY (the caller gates on it and on
//! `OXEN_HARNESS_CLASSIC_INPUT`). Everything here is best-effort and always
//! restored on drop.
//!
//! The file is split into focused submodules, leaving this module with the turn
//! orchestration and the [`Live`] state that ties them together:
//!
//! - [`composer`] — the pure line editor and recallable history.
//! - [`keys`] — keystroke classification (key → intent) and the shared line-edit op.
//! - [`layout`] — queue focus navigation and overflow-window planning.
//! - [`text`] — line rendering (windowing, word-wrap, the themed prompt).
//! - [`terminal`] — the raw-mode RAII guard and the input-forwarding thread.

mod composer;
mod keys;
mod layout;
mod terminal;
mod text;

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyEvent, KeyEventKind};
use harness_agent::{Agent, AgentError, AgentEvent};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::markdown::MarkdownStream;
use crate::queue::MessageQueue;
use crate::render::truncate;
use crate::theme::{LiveSpinner, Ui};

use composer::{Composer, History};
use keys::{apply_buf, classify_key, KeyAction, KeyIntent, Mode};
use layout::{queue_rows, Focus, QueueRow, MAX_QUEUE_ROWS, QUEUE_FRAME_ROWS};
use terminal::{
    region_bottom, resume_sequence, spawn_input, suspend_sequence, CrlfWriter, LiveTerminal,
};
use text::{composer_prompt, render_buffer, render_text_line, wrap_line};

/// Run `first_prompt`, then drain any queued prompts in order, all under a single
/// owned live terminal so the composer stays pinned across the whole sequence.
///
/// Returns `(end_session, draft)`: `end_session` is true when the user asked to
/// quit (Ctrl-C / Ctrl-D); `draft` is whatever unsent text was left in the
/// composer, so the caller can seed the idle prompt with it (a half-typed next
/// message isn't lost when the turn ends).
pub(crate) async fn run_prompt(
    agent: &mut Agent,
    first_prompt: &str,
    ui: &Ui,
    queue: &mut MessageQueue,
) -> Result<(bool, String)> {
    let term = LiveTerminal::new()?;
    let (rows, cols) = (term.rows, term.cols);

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
        s.status_line = Some(crate::context_usage_line(agent, ui));
        s.compression_line = crate::compression_status_line(agent, ui);
    }

    let mut next = Some(first_prompt.to_string());
    let mut exit = false;
    while let Some(prompt) = next.take() {
        match run_one_turn(agent, &prompt, &state, &mut rx, queue, &paused).await {
            TurnOutcome::Done(result) => {
                {
                    let mut s = state.borrow_mut();
                    if let Err(e) = &result {
                        s.print_line(&format!("  {}", ui.red(&ui.death())));
                        s.print_line(&format!(
                            "  {}",
                            ui.dim(&format!("The trail guide says: {e}"))
                        ));
                        if let Some(hint) = crate::auth_cmd::auth_hint(ui, &e.to_string()) {
                            s.print_line(&hint);
                        }
                    }
                    // Refresh the pinned meters (they sit above the divider,
                    // not in the scrollback) with the turn's totals.
                    s.status_line = Some(crate::context_usage_line(agent, ui));
                    s.compression_line = crate::compression_status_line(agent, ui);
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
                    next = Some(msg);
                }
            }
            TurnOutcome::Interrupted | TurnOutcome::Exit => {
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
    /// Ctrl-D / Ctrl-C on an empty box — end the session.
    Exit,
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

    let result = loop {
        match rx.recv().await {
            Some(Event::Key(key)) => {
                let action = state.borrow_mut().handle_key(key, queue.len());
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
                    KeyAction::Interrupt | KeyAction::Exit => break Idle::Exit,
                }
            }
            Some(Event::Resize(c, r)) => {
                state.borrow_mut().handle_resize(c, r, queue.items());
            }
            Some(Event::Paste(text)) => {
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
enum TurnOutcome {
    /// The turn finished on its own (success or agent error).
    Done(Result<String, AgentError>),
    /// Ctrl-C — interrupt and end the session.
    Interrupted,
    /// Ctrl-D on an empty composer — end the session.
    Exit,
}

/// Drive one `run_turn` to completion while servicing the composer: keystrokes
/// stack onto `queue`, the spinner advances on a tick, and every event redraws
/// the bottom line.
async fn run_one_turn(
    agent: &mut Agent,
    prompt: &str,
    state: &Rc<RefCell<Live>>,
    rx: &mut UnboundedReceiver<Event>,
    queue: &mut MessageQueue,
    paused: &Arc<AtomicBool>,
) -> TurnOutcome {
    // Pull any dropped files out of the line and announce them in the scroll
    // region before the turn starts.
    let (text, attachments, warnings) = crate::attach::extract_attachments(prompt);
    {
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
    }

    state.borrow_mut().begin_turn(queue.items());

    let cb = state.clone();
    let cb_paused = paused.clone();
    let turn = agent.run_turn_with_attachments(text, attachments, move |event| {
        cb.borrow_mut().on_event(event, &cb_paused);
    });
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
                                if !trimmed.is_empty() {
                                    queue.add(trimmed);
                                }
                                let mut s = state.borrow_mut();
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
                state.borrow_mut().tick_spinner();
            }
        }
    }
}

/// Blank rows kept between the agent's scrolling output and the pinned input
/// area, so the prompt always has at least one full line of breathing room.
const SPACER_ROWS: usize = 1;

/// Rows for the faint divider rule drawn just above the input area (matching the
/// idle prompt's separator).
const DIVIDER_ROWS: usize = 1;

/// Most input lines shown inside the box at once; beyond this it windows around
/// the caret so a long paste can't push the conversation off-screen.
const MAX_INPUT_ROWS: usize = 8;

/// Display previews are capped to this many characters when snapshotting the
/// queue, then windowed to the terminal width at paint time.
const PREVIEW_CAP: usize = 256;

/// The slash commands offered by Tab completion + the inline hint, with a short
/// description. Kept in sync with [`crate::repl::parse_command`].
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/model", "show or switch the model"),
    ("/theme", "change the theme"),
    ("/queue", "manage the message queue"),
    ("/loop", "run or list loops"),
    ("/export", "export the transcript"),
    ("/departing", "set the banner location"),
    ("/auth", "set your Oxen API key"),
    ("/compression", "switch context compression (off/audit/on)"),
    ("/help", "show help"),
    ("/exit", "quit"),
];

/// All mutable state shared between the turn callback and the event loop: the
/// streamed-output renderer, the spinner, the composer, and the navigable queue
/// list.
///
/// The [`MessageQueue`] itself stays owned by the event loop (the single source
/// of truth); `previews` is only a render snapshot of it, refreshed on every
/// change so the streaming callback can repaint the list without borrowing the
/// queue. Inline editing always reloads the full item text from the queue, so
/// the truncated previews are never edited.
struct Live {
    ui: Ui,
    cols: u16,
    rows: u16,
    out: io::Stdout,
    md: Option<MarkdownStream<CrlfWriter>>,
    spinner: Option<LiveSpinner>,
    /// While an interactive tool (the picker) owns the screen, we stop drawing.
    suspended: bool,
    /// The bottom composer's edit buffer.
    composer: Composer,
    /// Recallable input history (Up/Down at a line edge walk it).
    history: History,
    /// Where keyboard focus currently sits.
    focus: Focus,
    /// `Some` while inline-editing the focused item (loaded with its full text).
    edit: Option<Composer>,
    /// One-line, width-capped previews of the queued messages (a render cache).
    previews: Vec<String>,
    /// The last row carved for scrolling output; tracked so we only re-issue the
    /// DECSTBM region when the reserved bottom height actually changes.
    region_bottom: u16,
    /// Set when a `web_search` call failed for a missing Brave API key, so the
    /// caller can prompt for one once the composer hands back to cooked mode.
    needs_brave_key: bool,
    /// The context-usage trailer (`🧭 context …`), pinned just above the divider
    /// rather than printed into the scrollback — so it always sits right above
    /// the input area with the blank spacer separating it from the last message.
    status_line: Option<String>,
    /// The compression-savings line (`⊙ compression …`), pinned directly above
    /// [`Live::status_line`]. Updated in place on every `Compression` event
    /// instead of scrolling a new line into the conversation.
    compression_line: Option<String>,
    /// Slash-command / argument completion candidates for the current composer
    /// text (full-line replacements), shown as a hint above the box. Refreshed on
    /// every compose edit; empty when there's nothing to suggest.
    completion: Vec<String>,
    /// The candidate currently selected by Tab cycling (highlights the hint and
    /// drives menu-complete). `None` until the first Tab.
    comp_index: Option<usize>,
    /// Lazily-loaded model names (cloud catalog + installed local) for `/model`
    /// argument completion, cached so we don't rescan on every keystroke.
    model_names: Option<Vec<String>>,
}

impl Live {
    fn new(ui: Ui, cols: u16, rows: u16) -> Self {
        Self {
            ui,
            cols,
            rows,
            out: io::stdout(),
            md: None,
            spinner: None,
            suspended: false,
            composer: Composer::new(),
            history: History::default(),
            focus: Focus::Composer,
            edit: None,
            previews: Vec::new(),
            region_bottom: region_bottom(rows),
            needs_brave_key: false,
            status_line: None,
            compression_line: None,
            completion: Vec::new(),
            comp_index: None,
            model_names: None,
        }
    }

    // --- queue snapshot + focus -------------------------------------------

    /// The unsent text currently in the bottom composer. Carried back to the
    /// idle prompt when the composer hands off, so a half-typed next message
    /// survives the turn ending instead of being wiped.
    fn composer_draft(&self) -> String {
        self.composer.text()
    }

    /// Refresh the preview snapshot from the authoritative queue, then re-clamp
    /// focus (and drop any inline edit that lost its item).
    fn sync_queue(&mut self, items: &[String]) {
        self.previews = items.iter().map(|m| truncate(m, PREVIEW_CAP)).collect();
        self.focus = self.focus.clamp(self.previews.len());
        if self.focus.item().is_none() {
            self.edit = None;
        }
    }

    fn focused_item(&self) -> Option<usize> {
        self.focus.item()
    }

    fn mode(&self) -> Mode {
        if self.edit.is_some() {
            Mode::Edit
        } else if self.focus.item().is_some() {
            Mode::Browse
        } else {
            Mode::Compose
        }
    }

    /// Load the focused item's full `text` into the inline editor.
    fn begin_edit(&mut self, text: &str) {
        if self.focus.item().is_some() {
            self.edit = Some(Composer::seeded(text));
        }
    }

    /// Finish an inline edit, returning the focused item index and its new text.
    fn take_edit(&mut self) -> Option<(usize, String)> {
        let idx = self.focus.item()?;
        let mut e = self.edit.take()?;
        Some((idx, e.take()))
    }

    fn cancel_edit(&mut self) {
        self.edit = None;
    }

    // --- turn lifecycle ----------------------------------------------------

    /// Reset render state, snapshot the queue, and start the thinking spinner
    /// for a fresh turn.
    fn begin_turn(&mut self, items: &[String]) {
        self.sync_queue(items);
        self.md = None;
        self.begin_thinking();
        self.render();
    }

    /// Flush any open Markdown and stop the spinner at the end of a turn.
    fn finish(&mut self) {
        self.stop_spinner();
        self.end_markdown();
    }

    fn begin_thinking(&mut self) {
        self.stop_spinner();
        self.spinner = LiveSpinner::new(&self.ui, self.ui.thinking());
        self.draw_spinner();
    }

    /// Start the indicator shown *while assistant text streams in*, so a pause
    /// between tokens (or a long, not-yet-newline-terminated line like a code
    /// block) keeps animating with a running timer instead of looking frozen.
    fn begin_streaming(&mut self) {
        self.stop_spinner();
        self.spinner = LiveSpinner::new(&self.ui, self.ui.writing());
        self.draw_spinner();
    }

    fn begin_working(&mut self, tool: &str, target: Option<String>) {
        self.stop_spinner();
        self.spinner = LiveSpinner::with_target(&self.ui, self.ui.tool_verbs(tool), target);
        self.draw_spinner();
    }

    /// Erase the spinner's current line without dropping the spinner, so newly
    /// completed streamed output can be written where the spinner sat (it's then
    /// redrawn one line below via [`Live::draw_spinner`]).
    fn clear_spinner_line(&mut self) {
        if self.spinner.is_some() && !self.suspended {
            let _ = write!(self.out, "\r\x1b[K");
            let _ = self.out.flush();
        }
    }

    fn stop_spinner(&mut self) {
        if self.spinner.take().is_some() && !self.suspended {
            let _ = write!(self.out, "\r\x1b[K");
            let _ = self.out.flush();
        }
    }

    fn end_markdown(&mut self) {
        if let Some(mut md) = self.md.take() {
            md.finish();
        }
    }

    // --- event rendering (mirrors render::TurnRenderer) --------------------

    fn on_event(&mut self, event: &AgentEvent, paused: &Arc<AtomicBool>) {
        match event {
            AgentEvent::Token(t) => self.on_token(t),
            // The model started writing a canvas; surface it while its content
            // streams in (the full preview prints on ToolStart).
            AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => {
                self.on_canvas_pending(name)
            }
            AgentEvent::ToolPending { .. } => {}
            AgentEvent::ToolStart { name, arguments } => {
                self.on_tool_start(name, arguments, paused)
            }
            AgentEvent::ToolEnd { name, result } => self.on_tool_end(name, result, paused),
            // Usage is surfaced in the banner/status, not inline during a turn.
            AgentEvent::Usage { .. } => {}
            AgentEvent::Compacted { detail } => self.on_compacted(detail),
            AgentEvent::Compression {
                mode,
                saved_tokens,
                total_saved_tokens,
                ..
            } => self.on_compression(mode, *saved_tokens, *total_saved_tokens),
            // Streaming tool-argument fragments drive the desktop UI only.
            AgentEvent::ToolDelta { .. } => {}
        }
    }

    fn on_token(&mut self, t: &str) {
        if self.md.is_none() {
            self.stop_spinner();
            self.write_region("\n");
            self.md = Some(MarkdownStream::new(self.ui.clone(), CrlfWriter::new()));
            // Keep a live indicator going *below* the streamed text so a pause
            // mid-response (or a long, not-yet-complete line such as a code block)
            // never looks frozen.
            self.begin_streaming();
        } else {
            // Clear the trailing spinner line before emitting newly completed
            // markdown lines, so output and spinner don't collide.
            self.clear_spinner_line();
        }
        if let Some(md) = self.md.as_mut() {
            md.push(t);
        }
        // Redraw the spinner on the fresh line just below the output.
        self.draw_spinner();
        self.render_composer();
    }

    fn on_canvas_pending(&mut self, name: &str) {
        self.stop_spinner();
        self.end_markdown();
        self.write_region(&format!(
            "  {} {}\n",
            self.ui.green("📄"),
            self.ui.dim("writing canvas…")
        ));
        self.begin_working(name, None);
        self.render_composer();
    }

    fn on_tool_start(&mut self, name: &str, arguments: &str, paused: &Arc<AtomicBool>) {
        self.stop_spinner();
        self.end_markdown();
        // The picker draws its own UI and reads keys, so hand the screen over to
        // it instead of printing a tool line + spinner.
        if name == harness_tools::ASK_USER_TOOL {
            self.suspend(paused);
            return;
        }
        let target = tool_target(arguments);
        // For a canvas, preview the document inline; the result line then reports
        // the saved path / browser open.
        if name == harness_tools::CANVAS_TOOL {
            if let Some(block) = crate::canvas::render_canvas_block(&self.ui, arguments) {
                self.write_region(&format!("{}\n", block.join("\n")));
            }
            self.begin_working(name, target);
            self.render_composer();
            return;
        }
        // For file writes/edits, show a colored diff instead of the generic
        // one-line tool preview.
        if let Some(block) = crate::diff::render_file_change(&self.ui, name, arguments) {
            self.write_region(&format!("{}\n", block.join("\n")));
        } else {
            let verbs = self.ui.tool_verbs(name);
            let verb = verbs.first().map(String::as_str).unwrap_or("Working");
            let line = format!(
                "  {} {}  {}",
                self.ui.green("◆"),
                self.ui.accent(verb),
                self.ui
                    .dim(&format!("{name}({})", truncate(arguments, 100))),
            );
            self.write_region(&format!("{line}\n"));
        }
        self.begin_working(name, target);
        self.render_composer();
    }

    fn on_tool_end(&mut self, name: &str, result: &str, paused: &Arc<AtomicBool>) {
        self.stop_spinner();
        if name == harness_tools::ASK_USER_TOOL {
            self.resume(paused);
            self.begin_thinking();
            // Repaint the full list: the picker drew over the screen.
            self.render_forcing_region();
            return;
        }
        // Web search with no key: flag it for a prompt once the composer hands
        // back to cooked mode, and show a friendlier line.
        if name == harness_tools::WEB_SEARCH_TOOL
            && result.contains(harness_tools::web::WEB_SEARCH_NO_KEY)
        {
            self.needs_brave_key = true;
            let line = format!(
                "  {} {}",
                self.ui.brown("└─"),
                self.ui
                    .dim("no Brave API key — you'll be prompted to add one below"),
            );
            self.write_region(&format!("{line}\n"));
            self.begin_thinking();
            self.render_composer();
            return;
        }
        let line = format!(
            "  {} {}",
            self.ui.brown("└─"),
            self.ui.dim(&truncate(result, 140)),
        );
        self.write_region(&format!("{line}\n"));
        self.begin_thinking();
        self.render_composer();
    }

    fn on_compacted(&mut self, detail: &str) {
        self.stop_spinner();
        let line = format!(
            "  {} {}",
            self.ui.brown("⊙"),
            self.ui.dim(&format!("compacted context — {detail}")),
        );
        self.write_region(&format!("{line}\n"));
        self.begin_thinking();
        self.render_composer();
    }

    fn on_compression(&mut self, mode: &str, saved_tokens: usize, total_saved_tokens: usize) {
        // Update the pinned line (above the context meter) in place — the
        // savings are chrome, not conversation, so they never scroll a line
        // into the transcript between tool output and the spinner.
        let verb = if mode == "audit" {
            "would save"
        } else {
            "saved"
        };
        self.compression_line = Some(format!(
            "  {} {}",
            self.ui.brown("⊙"),
            self.ui.dim(&format!(
                "compression {verb} ~{saved_tokens} tokens this call ({total_saved_tokens} total)"
            )),
        ));
        self.render();
    }

    // --- spinner -----------------------------------------------------------

    fn tick_spinner(&mut self) {
        if self.suspended {
            return;
        }
        if let Some(sp) = self.spinner.as_mut() {
            sp.tick();
        }
        self.draw_spinner();
    }

    fn draw_spinner(&mut self) {
        if self.suspended {
            return;
        }
        if let Some(sp) = self.spinner.as_ref() {
            let line = sp.line();
            let _ = write!(self.out, "\r{line}\x1b[K");
            let _ = self.out.flush();
            self.render_composer();
        }
    }

    // --- writing into the scroll region ------------------------------------

    /// Write text into the scroll region (newlines become `\r\n`), where the
    /// output cursor lives. Used for tool lines and the blank separator.
    fn write_region(&mut self, text: &str) {
        let mut w = CrlfWriter::new();
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
    }

    /// Print a complete status line into the region, then redraw the composer.
    /// Used for the post-turn context-usage trailer and drain announcements.
    fn print_line(&mut self, line: &str) {
        self.write_region(&format!("{line}\n"));
        self.render_composer();
    }

    // --- composer + queue list painting ------------------------------------

    /// Repaint the input area (used during streaming and after each keystroke).
    /// The box can change height as lines are added/removed, which re-carves the
    /// scroll region, so this defers to the full [`Live::paint`] — bracketed by
    /// save/restore-cursor, it leaves the streaming output position untouched.
    fn render_composer(&mut self) {
        self.paint(false);
    }

    /// Repaint the whole bottom area — the stacked queue list plus the composer
    /// — re-carving the scroll region only when the reserved height changed.
    fn render(&mut self) {
        self.paint(false);
    }

    /// Like [`Live::render`] but unconditionally re-issues the scroll region —
    /// used after a resize or after reclaiming the screen from the picker, where
    /// the terminal's region no longer matches our state.
    fn render_forcing_region(&mut self) {
        self.paint(true);
    }

    fn paint(&mut self, force_region: bool) {
        if self.suspended {
            return;
        }
        let len = self.previews.len();
        // Reserve frame rows up front so the header/footer borders never push the
        // last line of streamed output off-screen.
        let frame = if len == 0 { 0 } else { QUEUE_FRAME_ROWS };
        let plan = queue_rows(len, self.focus, self.rows, MAX_QUEUE_ROWS, frame);
        let chrome = if plan.is_empty() { 0 } else { QUEUE_FRAME_ROWS };
        // The input area's height grows with the lines typed.
        let box_lines = self.composer_box_lines();
        let box_h = box_lines.len() as u16;
        // Between the agent's output and the pinned input area: a blank spacer,
        // the compression savings (when active), the context-usage status, then
        // a faint divider rule (output · blank · compression · status · rule ·
        // input), so the prompt always has breathing room and a clear edge, and
        // the meters sit right above the input instead of trailing the last
        // message.
        let status_rows: u16 = self.compression_line.is_some() as u16
            + self.status_line.is_some() as u16;
        let reserved = (plan.len() + chrome) as u16
            + SPACER_ROWS as u16
            + status_rows
            + DIVIDER_ROWS as u16
            + box_h;
        let new_bottom = self.rows.saturating_sub(reserved).max(1);

        let mut buf = String::new();
        if force_region || new_bottom != self.region_bottom {
            // On an incremental change, clear the rows that move between the
            // output region and the reserved area so no stale text lingers.
            if !force_region {
                let lo = self.region_bottom.min(new_bottom) + 1;
                for r in lo..=self.rows {
                    buf.push_str(&format!("\x1b[{r};1H\x1b[2K"));
                }
            }
            // Re-carve the region and park the output cursor at its new bottom.
            buf.push_str(&format!("\x1b[1;{new_bottom}r\x1b[{new_bottom};1H"));
            self.region_bottom = new_bottom;
        }

        // Paint the framed queue table + composer below the region, bracketed by
        // save/restore so the output cursor inside the region is left undisturbed.
        buf.push_str("\x1b7");
        // Keep the spacer row(s) directly below the output region blank.
        for s in 0..SPACER_ROWS as u16 {
            buf.push_str(&format!("\x1b[{};1H\x1b[2K", new_bottom + 1 + s));
        }
        // The compression savings and context-usage meters sit under the
        // spacer, just above the divider (compression on top of context).
        let mut next_row = new_bottom + 1 + SPACER_ROWS as u16;
        for line in [&self.compression_line, &self.status_line]
            .into_iter()
            .flatten()
        {
            buf.push_str(&format!("\x1b[{next_row};1H\x1b[2K{line}"));
            next_row += 1;
        }
        // Then a faint full-width divider rule, just above the input area.
        let divider_row = next_row;
        buf.push_str(&format!(
            "\x1b[{divider_row};1H\x1b[2K{}",
            self.ui.dim(&"─".repeat(self.cols as usize))
        ));
        if !plan.is_empty() {
            let box_w = self.queue_box_w();
            let mut r = divider_row + DIVIDER_ROWS as u16;
            buf.push_str(&format!("\x1b[{r};1H\x1b[2K{}", self.queue_header(box_w)));
            for row in &plan {
                r += 1;
                buf.push_str(&format!(
                    "\x1b[{r};1H\x1b[2K{}",
                    self.queue_row_line(*row, box_w)
                ));
            }
            r += 1;
            buf.push_str(&format!("\x1b[{r};1H\x1b[2K{}", self.queue_footer(box_w)));
        }
        // The input box occupies the bottom `box_h` rows, pinned to row H.
        let box_start = self.rows.saturating_sub(box_h).saturating_add(1);
        for (i, line) in box_lines.iter().enumerate() {
            let row = box_start + i as u16;
            buf.push_str(&format!("\x1b[{row};1H\x1b[2K{line}"));
        }
        buf.push_str("\x1b8");
        let _ = write!(self.out, "{buf}");
        let _ = self.out.flush();
    }

    /// The inner content width of the queue table — the columns available
    /// *between* the `│ ` and ` │` of a framed row. Two-space left margin plus
    /// the four border/padding columns are reserved out of the terminal width.
    fn queue_box_w(&self) -> usize {
        (self.cols as usize).saturating_sub(6).max(8)
    }

    /// The table's top border, embedding the `Queued` title:
    /// `┌─ Queued ───────┐`. The title is accented; the rule is brown.
    fn queue_header(&self, box_w: usize) -> String {
        let label = "Queued";
        // `┌─ ` (3) + label + ` ` (1) + fill + `┐` (1) must span `box_w + 4`
        // columns to align with the framed rows below, so fill = box_w - 1 - len.
        let fill = box_w.saturating_sub(1 + label.chars().count());
        format!(
            "  {}{}{}",
            self.ui.brown("┌─ "),
            self.ui.accent(label),
            self.ui.brown(&format!(" {}┐", "─".repeat(fill))),
        )
    }

    /// The table's bottom border, embedding a key hint for what you can do with
    /// the queue right now — so editing a queued prompt is discoverable, not
    /// just deleting it. The hint is mode-aware (`↑ edit queued` while composing,
    /// `enter edit · d delete` while browsing, `enter save · esc cancel` while
    /// editing) and embeds in the rule with the same geometry as the header so
    /// the frame stays aligned; on a terminal too narrow to fit it, it falls back
    /// to a plain border.
    fn queue_footer(&self, box_w: usize) -> String {
        let hint = match self.mode() {
            Mode::Edit => "enter save · esc cancel",
            Mode::Browse => "enter edit · d delete",
            Mode::Compose => "↑ edit queued",
        };
        if hint.chars().count() < box_w {
            let fill = box_w.saturating_sub(1 + hint.chars().count());
            format!(
                "  {}{}{}",
                self.ui.brown("└─ "),
                self.ui.dim(hint),
                self.ui.brown(&format!(" {}┘", "─".repeat(fill))),
            )
        } else {
            format!(
                "  {}",
                self.ui.brown(&format!("└{}┘", "─".repeat(box_w + 2)))
            )
        }
    }

    /// Render one framed table row (`│ … │`): an `…(+k more)` overflow marker, a
    /// dimmed preview, the reverse-video highlighted focused item, or the live
    /// inline editor. Every cell is padded to `box_w` so the right border lines up.
    fn queue_row_line(&self, row: QueueRow, box_w: usize) -> String {
        let bar = self.ui.brown("│");
        match row {
            QueueRow::More(k) => {
                let text = format!("…(+{k} more)");
                let pad = box_w.saturating_sub(text.chars().count());
                format!("  {bar} {}{} {bar}", self.ui.dim(&text), " ".repeat(pad))
            }
            QueueRow::Item(idx) => {
                let num = idx + 1;
                let focused = self.focus == Focus::Item(idx);
                if focused {
                    if let Some(edit) = self.edit.as_ref() {
                        let prefix = format!("✎ {num}. ");
                        let prefix_w = prefix.chars().count();
                        // Leave a column for the caret so the editor never spills
                        // past the right border.
                        let avail = box_w.saturating_sub(prefix_w + 1);
                        let (body, width) = render_buffer(edit, avail, true);
                        let pad = box_w.saturating_sub(prefix_w + width);
                        return format!(
                            "  {bar} {}{}{} {bar}",
                            self.ui.accent(&prefix),
                            body,
                            " ".repeat(pad),
                        );
                    }
                    let plain = self.item_text(idx, num, box_w);
                    let pad = box_w.saturating_sub(plain.chars().count());
                    // Plain text under reverse video (no nested color codes).
                    format!("  {bar} \x1b[7m{plain}{}\x1b[0m {bar}", " ".repeat(pad))
                } else {
                    let prefix = format!("{num}. ");
                    let preview =
                        self.item_preview(idx, box_w.saturating_sub(prefix.chars().count()));
                    let pad =
                        box_w.saturating_sub(prefix.chars().count() + preview.chars().count());
                    format!(
                        "  {bar} {}{}{} {bar}",
                        self.ui.accent(&prefix),
                        self.ui.cream(&preview),
                        " ".repeat(pad),
                    )
                }
            }
        }
    }

    /// The numbered, width-fitted plain text for an item (`3. fix the bug`),
    /// clamped so it never exceeds the table's content width.
    fn item_text(&self, idx: usize, num: usize, box_w: usize) -> String {
        let prefix = format!("{num}. ");
        format!(
            "{prefix}{}",
            self.item_preview(idx, box_w.saturating_sub(prefix.chars().count()))
        )
    }

    /// A queued message preview fitted to `width` columns, leaving room for the
    /// `…` truncation marker so the result never overflows the cell.
    fn item_preview(&self, idx: usize, width: usize) -> String {
        let preview = self.previews.get(idx).map(String::as_str).unwrap_or("");
        truncate(preview, width.saturating_sub(1))
    }

    /// Render the input rows — deliberately **frameless** (no bordered box), so
    /// a terminal selection over the bottom of the screen never picks up border
    /// characters. Long lines **word-wrap** onto the next visual row (rather
    /// than scrolling sideways); the themed prompt sits on the first row and
    /// wrapped/continuation rows align under it. The area grows with the rows
    /// typed (capped at [`MAX_INPUT_ROWS`], windowing around the caret beyond
    /// that). The caret (reverse-video cell) shows only when the composer holds
    /// focus — not while browsing/editing the queue.
    fn composer_box_lines(&self) -> Vec<String> {
        let depth = self.previews.len();
        let (plain_prompt, styled_prompt) = composer_prompt(&self.ui, depth);
        let prompt_w = plain_prompt.chars().count();
        let box_w = self.queue_box_w();
        // Wrap width leaves a column for the caret so it never spills past the
        // right edge; every row is indented under the prompt for alignment.
        let avail = box_w.saturating_sub(prompt_w + 1).max(4);
        let caret_on = matches!(self.focus, Focus::Composer) && self.edit.is_none();

        let lines: Vec<Vec<char>> = self
            .composer
            .lines()
            .into_iter()
            .map(|l| l.chars().collect())
            .collect();
        let (caret_line, caret_col) = self.composer.line_col();

        // Word-wrap each logical line into visual rows, tracking which visual row
        // + column the caret lands on.
        let mut vrows: Vec<Vec<char>> = Vec::new();
        let mut caret_vrow = 0usize;
        let mut caret_vcol = 0usize;
        for (li, line) in lines.iter().enumerate() {
            for (start, chunk) in wrap_line(line, avail) {
                let len = chunk.len();
                let owns_caret = caret_on
                    && li == caret_line
                    && ((caret_col >= start && caret_col < start + len)
                        || (caret_col == start + len && start + len == line.len()));
                if owns_caret {
                    caret_vrow = vrows.len();
                    caret_vcol = caret_col - start;
                }
                vrows.push(chunk);
            }
        }

        // Window the visible rows around the caret when there are more than fit.
        let total = vrows.len();
        let (lo, hi) = if total <= MAX_INPUT_ROWS {
            (0, total)
        } else {
            let lo = caret_vrow
                .saturating_sub(MAX_INPUT_ROWS / 2)
                .min(total - MAX_INPUT_ROWS);
            (lo, lo + MAX_INPUT_ROWS)
        };

        let mut out: Vec<String> = Vec::new();
        // A slash-command / argument suggestion hint sits just above the input
        // while composing at idle (no spinner), so completions are discoverable.
        if self.spinner.is_none()
            && matches!(self.focus, Focus::Composer)
            && self.edit.is_none()
            && !self.completion.is_empty()
        {
            out.push(self.completion_hint(box_w));
        }
        for (vi, row) in vrows.iter().enumerate().take(hi).skip(lo) {
            let caret = (caret_on && vi == caret_vrow).then_some(caret_vcol);
            let (body, _) = render_text_line(row, caret, avail);
            let prefix = if vi == 0 {
                styled_prompt.clone()
            } else {
                " ".repeat(prompt_w)
            };
            out.push(format!("  {prefix}{body}"));
        }
        out
    }

    /// One-line suggestion hint shown above the box: the candidate tokens (the
    /// command, or model name for `/model` args), the Tab-selected one
    /// highlighted, capped to the box width with an `…` overflow marker.
    fn completion_hint(&self, box_w: usize) -> String {
        let budget = box_w.saturating_sub(6); // room for the icon + a trailing "⇥"
        let mut shown = String::new();
        let mut width = 0usize;
        let mut shown_any = false;
        for (i, cand) in self.completion.iter().enumerate() {
            let label = cand.rsplit(' ').next().unwrap_or(cand);
            let label_w = label.chars().count();
            // +1 for the separating space between chips.
            let need = label_w + if shown_any { 1 } else { 0 };
            if shown_any && width + need > budget {
                shown.push_str(&self.ui.dim(" …"));
                break;
            }
            if shown_any {
                shown.push(' ');
                width += 1;
            }
            if Some(i) == self.comp_index {
                shown.push_str(&format!("\x1b[7m{label}\x1b[0m"));
            } else {
                shown.push_str(&self.ui.cream(label));
            }
            width += label_w;
            shown_any = true;
        }
        format!("  {} {}", self.ui.brown("⇥"), shown)
    }

    // --- slash-command + argument completion -------------------------------

    /// Candidate full-line replacements for the current composer text: slash
    /// commands while typing the command word, or model names after `/model `.
    /// Empty when the text isn't a completable `/command`.
    fn compute_candidates(&mut self) -> Vec<String> {
        let text = self.composer.text();
        if !text.starts_with('/') {
            return Vec::new();
        }
        let mut parts = text.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        match parts.next() {
            // Still typing the command word — match against the command list.
            None => SLASH_COMMANDS
                .iter()
                .filter(|(c, _)| c.starts_with(cmd))
                .map(|(c, _)| c.to_string())
                .collect(),
            // `/model <partial>` — complete model names (cloud + local).
            Some(arg) if cmd == "/model" => {
                let needle = arg.trim().to_lowercase();
                self.model_candidates()
                    .into_iter()
                    .filter(|m| m.to_lowercase().starts_with(&needle))
                    .map(|m| format!("/model {m}"))
                    .collect()
            }
            // `/compression <partial>` — complete the three modes.
            Some(arg) if cmd == "/compression" || cmd == "/compress" => {
                let needle = arg.trim().to_lowercase();
                ["off", "audit", "on"]
                    .iter()
                    .filter(|m| m.starts_with(&needle))
                    .map(|m| format!("{cmd} {m}"))
                    .collect()
            }
            Some(_) => Vec::new(),
        }
    }

    /// Model names for `/model` completion: the cloud catalog plus installed
    /// local models, loaded once and cached.
    fn model_candidates(&mut self) -> Vec<String> {
        if self.model_names.is_none() {
            let mut names: Vec<String> = harness_runtime::models::catalog()
                .into_iter()
                .map(|m| m.id)
                .collect();
            if let Ok(store) = harness_local::ModelStore::open() {
                names.extend(store.installed().into_iter().map(|m| m.id));
            }
            names.sort();
            names.dedup();
            self.model_names = Some(names);
        }
        self.model_names.clone().unwrap_or_default()
    }

    /// Recompute the completion hint after a compose-buffer change, and drop any
    /// in-progress Tab cycle (the candidates may have changed).
    fn refresh_completion(&mut self) {
        self.completion = self.compute_candidates();
        self.comp_index = None;
    }

    /// Handle Tab: menu-complete the composer to the next matching candidate,
    /// cycling on repeated presses. Returns whether anything changed.
    fn complete(&mut self) -> bool {
        if self.completion.is_empty() {
            self.completion = self.compute_candidates();
        }
        if self.completion.is_empty() {
            return false;
        }
        let next = match self.comp_index {
            // Still on the last pick (no edits since) → advance to cycle.
            Some(i) if self.composer.text() == self.completion[i] => {
                (i + 1) % self.completion.len()
            }
            _ => 0,
        };
        self.composer.set_text(&self.completion[next]);
        self.comp_index = Some(next);
        true
    }

    /// Translate a keystroke into a [`KeyAction`], mutating the composer / focus
    /// / inline-edit buffer in place. Queue mutations are deferred to the loop.
    fn handle_key(&mut self, key: KeyEvent, queue_len: usize) -> KeyAction {
        // Windows reports key releases too; act only on presses.
        if key.kind != KeyEventKind::Press {
            return KeyAction::None;
        }
        match classify_key(
            key.code,
            key.modifiers,
            self.mode(),
            self.composer.is_empty(),
        ) {
            KeyIntent::Ignore => KeyAction::None,
            KeyIntent::Interrupt => KeyAction::Interrupt,
            KeyIntent::Exit => KeyAction::Exit,
            KeyIntent::Compose(op) => {
                apply_buf(&mut self.composer, op);
                // Editing leaves history recall — keep the buffer as the draft.
                self.history.reset();
                self.refresh_completion();
                KeyAction::Redraw
            }
            KeyIntent::ComposeNewline => {
                self.composer.insert_newline();
                self.history.reset();
                self.refresh_completion();
                KeyAction::Redraw
            }
            KeyIntent::Complete => {
                if self.complete() {
                    KeyAction::Redraw
                } else {
                    KeyAction::None
                }
            }
            KeyIntent::ComposerSubmit => {
                let text = self.composer.take();
                self.history.push(&text);
                self.completion.clear();
                self.comp_index = None;
                KeyAction::Submit(text)
            }
            KeyIntent::ComposeUp => {
                // Move up a line if there is one; on the first line, either focus
                // the queue (empty box) or recall the previous history entry.
                if self.composer.move_up() {
                    self.history.reset();
                } else if self.composer.is_empty() && queue_len > 0 {
                    self.focus = self.focus.up(queue_len);
                } else {
                    let draft = self.composer.text();
                    if let Some(prev) = self.history.prev(&draft) {
                        self.composer.set_text(&prev);
                    }
                }
                self.refresh_completion();
                KeyAction::Redraw
            }
            KeyIntent::ComposeDown => {
                // Move down a line if there is one; on the last line, recall the
                // next history entry (eventually restoring the stashed draft).
                if self.composer.move_down() {
                    self.history.reset();
                } else if let Some(next) = self.history.next() {
                    self.composer.set_text(&next);
                }
                self.refresh_completion();
                KeyAction::Redraw
            }
            KeyIntent::FocusUp => {
                self.focus = self.focus.up(queue_len);
                // Leaving the composer for the queue drops the suggestion hint.
                self.completion.clear();
                self.comp_index = None;
                KeyAction::Redraw
            }
            KeyIntent::FocusDown => {
                self.focus = self.focus.down(queue_len);
                KeyAction::Redraw
            }
            KeyIntent::BeginEdit => KeyAction::BeginEdit,
            KeyIntent::DeleteItem => KeyAction::DeleteFocused,
            KeyIntent::Edit(op) => {
                if let Some(edit) = self.edit.as_mut() {
                    apply_buf(edit, op);
                }
                KeyAction::Redraw
            }
            KeyIntent::EditCommit => KeyAction::SaveEdit,
            KeyIntent::EditCancel => KeyAction::CancelEdit,
        }
    }

    /// Insert pasted / drag-dropped text into whichever single-line editor has
    /// focus (the inline item editor while editing, otherwise the composer),
    /// flattening newlines to spaces. Bracketed paste delivers a drop as one
    /// block, so a path with escaped spaces lands intact.
    fn insert_paste(&mut self, text: &str) {
        let target = self.edit.as_mut().unwrap_or(&mut self.composer);
        for ch in text.chars() {
            let ch = if ch == '\n' || ch == '\r' { ' ' } else { ch };
            target.insert_char(ch);
        }
        if self.edit.is_none() {
            self.refresh_completion();
        }
    }

    // --- resize + interactive-tool hand-off --------------------------------

    fn handle_resize(&mut self, cols: u16, rows: u16, items: &[String]) {
        self.cols = cols;
        self.rows = rows;
        self.sync_queue(items);
        self.render_forcing_region();
        self.draw_spinner();
    }

    /// Hand the terminal to an interactive tool (the picker): drop the scroll
    /// region, show the cursor, leave raw mode, and pause input forwarding.
    ///
    /// Crucially we leave the cursor on a *fresh bottom line* before handing off.
    /// Resetting the scroll region (`\x1b[r`) homes the cursor to the top of the
    /// screen, so without repositioning the picker would draw its first frame at
    /// the top — out of view from where the user is looking — and only become
    /// visible once a keypress forced a redraw.
    fn suspend(&mut self, paused: &Arc<AtomicBool>) {
        self.suspended = true;
        paused.store(true, Ordering::Relaxed);
        let _ = write!(
            self.out,
            "{}",
            suspend_sequence(self.region_bottom, self.rows)
        );
        let _ = self.out.flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }

    /// Reclaim the terminal after the interactive tool finishes.
    fn resume(&mut self, paused: &Arc<AtomicBool>) {
        let _ = crossterm::terminal::enable_raw_mode();
        self.suspended = false;
        paused.store(false, Ordering::Relaxed);
        let _ = write!(self.out, "{}", resume_sequence(self.rows));
        let _ = self.out.flush();
        // `resume_sequence` carved a composer-only region; record it so the
        // follow-up full repaint re-carves to fit the queue list.
        self.region_bottom = region_bottom(self.rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn live(cols: u16, rows: u16) -> Live {
        Live::new(
            Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default())),
            cols,
            rows,
        )
    }

    fn plain_live(cols: u16, rows: u16) -> Live {
        Live::new(
            Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default())),
            cols,
            rows,
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    // --- slash-command completion ------------------------------------------

    #[test]
    fn typing_a_slash_offers_command_suggestions() {
        let mut l = live(80, 24);
        l.handle_key(key(KeyCode::Char('/')), 0);
        // Every slash command is suggested right after `/`.
        assert!(l.completion.iter().any(|c| c == "/model"));
        assert!(l.completion.iter().any(|c| c == "/theme"));
        // Narrowing filters the list.
        l.handle_key(key(KeyCode::Char('m')), 0);
        assert_eq!(l.completion, vec!["/model".to_string()]);
    }

    #[test]
    fn tab_completes_and_cycles_command_matches() {
        let mut l = live(80, 24);
        for ch in "/e".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        // `/export` and `/exit` both match; Tab menu-completes, then cycles.
        l.handle_key(key(KeyCode::Tab), 0);
        let first = l.composer.text();
        l.handle_key(key(KeyCode::Tab), 0);
        let second = l.composer.text();
        assert_ne!(first, second);
        assert!(first.starts_with("/e") && second.starts_with("/e"));
    }

    #[test]
    fn editing_after_complete_drops_the_cycle() {
        let mut l = live(80, 24);
        l.handle_key(key(KeyCode::Char('/')), 0);
        l.handle_key(key(KeyCode::Char('h')), 0);
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/help");
        // A normal edit clears the in-progress cycle selection.
        l.handle_key(key(KeyCode::Char('x')), 0);
        assert_eq!(l.comp_index, None);
    }

    // --- Live wiring (no TTY: handle_key + buffer state, never paint) ------

    #[test]
    fn handle_key_navigates_between_composer_and_list() {
        let mut l = live(80, 24);
        l.sync_queue(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(l.focus, Focus::Composer);
        assert!(matches!(
            l.handle_key(key(KeyCode::Up), 3),
            KeyAction::Redraw
        ));
        assert_eq!(l.focus, Focus::Item(2)); // nearest item
        l.handle_key(key(KeyCode::Up), 3);
        l.handle_key(key(KeyCode::Up), 3);
        assert_eq!(l.focus, Focus::Item(0));
        l.handle_key(key(KeyCode::Up), 3); // clamps at the top
        assert_eq!(l.focus, Focus::Item(0));
        l.focus = Focus::Item(2);
        l.handle_key(key(KeyCode::Down), 3); // past the last item
        assert_eq!(l.focus, Focus::Composer);
    }

    #[test]
    fn inline_edit_saves_the_full_text() {
        let mut l = live(80, 24);
        l.sync_queue(&["fix bug".into()]);
        l.focus = Focus::Item(0);
        assert!(matches!(
            l.handle_key(key(KeyCode::Enter), 1),
            KeyAction::BeginEdit
        ));
        l.begin_edit("fix bug"); // the loop seeds from the full queue text
        assert_eq!(l.mode(), Mode::Edit);
        for ch in " now".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 1);
        }
        assert!(matches!(
            l.handle_key(key(KeyCode::Enter), 1),
            KeyAction::SaveEdit
        ));
        assert_eq!(l.take_edit(), Some((0, "fix bug now".to_string())));
    }

    #[test]
    fn inline_edit_cancel_discards_changes() {
        let mut l = live(80, 24);
        l.sync_queue(&["original".into()]);
        l.focus = Focus::Item(0);
        l.begin_edit("original");
        l.handle_key(key(KeyCode::Char('X')), 1);
        assert!(matches!(
            l.handle_key(key(KeyCode::Esc), 1),
            KeyAction::CancelEdit
        ));
        l.cancel_edit();
        assert!(l.edit.is_none());
        // The queue snapshot is untouched (the queue itself never changed).
        assert_eq!(l.previews, vec!["original".to_string()]);
    }

    #[test]
    fn delete_signals_the_loop_and_sync_reclamps_focus() {
        let mut l = live(80, 24);
        l.sync_queue(&["a".into(), "b".into()]);
        l.focus = Focus::Item(1);
        assert!(matches!(
            l.handle_key(key(KeyCode::Char('d')), 2),
            KeyAction::DeleteFocused
        ));
        // The loop removes item 2; emulate the resulting re-sync.
        l.sync_queue(&["a".into()]);
        assert_eq!(l.focus, Focus::Item(0));
        // Removing the last item drops focus back to the composer.
        l.sync_queue(&[]);
        assert_eq!(l.focus, Focus::Composer);
    }

    #[test]
    fn ctrl_d_exits_only_on_an_empty_composer() {
        let mut l = live(80, 24);
        assert!(matches!(
            l.handle_key(ctrl(KeyCode::Char('d')), 0),
            KeyAction::Exit
        ));
        l.handle_key(key(KeyCode::Char('x')), 0);
        assert!(matches!(
            l.handle_key(ctrl(KeyCode::Char('d')), 0),
            KeyAction::None
        ));
    }

    // --- queue table painting ----------------------------------------------

    #[test]
    fn queue_table_header_shows_title_and_aligns_with_rows() {
        let mut l = plain_live(60, 24);
        l.sync_queue(&[
            "fix the parser".into(),
            "add tests".into(),
            "write docs".into(),
        ]);
        let box_w = l.queue_box_w();

        let header = l.queue_header(box_w);
        let footer = l.queue_footer(box_w);
        let row = l.queue_row_line(QueueRow::Item(0), box_w);

        // The header carries the "Queued" title and the box-drawing corners.
        assert!(header.contains("Queued"), "header: {header:?}");
        assert!(header.trim_start().starts_with("┌─ "), "header: {header:?}");
        assert!(footer.trim_start().starts_with('└'), "footer: {footer:?}");
        assert!(footer.trim_end().ends_with('┘'), "footer: {footer:?}");
        // The item is framed and numbered.
        assert!(row.contains("│"), "row: {row:?}");
        assert!(row.contains("1. fix the parser"), "row: {row:?}");

        // With color disabled the only escapes are reverse-video on focus, so
        // every framed line is the same visible width as the borders.
        let width = |s: &str| s.chars().count();
        assert_eq!(width(&header), width(&row), "header vs row width");
        assert_eq!(width(&footer), width(&row), "footer vs row width");
    }

    #[test]
    fn queue_footer_hint_surfaces_edit_per_mode() {
        let mut l = plain_live(60, 24);
        l.sync_queue(&["fix the parser".into()]);
        let box_w = l.queue_box_w();

        // Composing with a queue: point the user up into it.
        assert!(l.queue_footer(box_w).contains("↑ edit queued"));

        // Browsing a focused item: edit is offered alongside delete.
        l.focus = Focus::Item(0);
        let browsing = l.queue_footer(box_w);
        assert!(browsing.contains("enter edit"), "footer: {browsing:?}");
        assert!(browsing.contains("d delete"), "footer: {browsing:?}");

        // Inline-editing: save + cancel.
        l.begin_edit("fix the parser");
        let editing = l.queue_footer(box_w);
        assert!(editing.contains("enter save"), "footer: {editing:?}");
        assert!(editing.contains("esc cancel"), "footer: {editing:?}");
    }

    #[test]
    fn focused_and_overflow_rows_keep_the_right_border_aligned() {
        let mut l = plain_live(50, 24);
        l.sync_queue(&(0..3).map(|i| format!("task {i}")).collect::<Vec<_>>());
        let box_w = l.queue_box_w();
        let target = l.queue_row_line(QueueRow::Item(1), box_w).chars().count();

        l.focus = Focus::Item(1);
        let focused = l.queue_row_line(QueueRow::Item(1), box_w);
        // Reverse-video escapes don't count toward visible width, so trim them.
        let visible = focused.replace("\x1b[7m", "").replace("\x1b[0m", "");
        assert_eq!(visible.chars().count(), target, "focused row width");

        let more = l.queue_row_line(QueueRow::More(4), box_w);
        assert!(more.contains("…(+4 more)"), "more: {more:?}");
        assert_eq!(more.chars().count(), target, "overflow row width");
    }

    // --- paste --------------------------------------------------------------

    #[test]
    fn paste_inserts_into_the_composer_and_flattens_newlines() {
        let mut l = live(80, 24);
        // A drag-dropped path (with a trailing newline the terminal appends).
        l.insert_paste("/tmp/My\\ Shot.png\n");
        assert_eq!(l.composer.take(), "/tmp/My\\ Shot.png ");
    }

    #[test]
    fn paste_targets_the_inline_editor_while_editing() {
        let mut l = live(80, 24);
        l.sync_queue(&["x".into()]);
        l.focus = Focus::Item(0);
        l.begin_edit("x");
        l.insert_paste("yz");
        assert_eq!(l.take_edit(), Some((0, "xyz".to_string())));
    }

    #[test]
    fn empty_queue_keeps_focus_on_composer() {
        let mut l = live(80, 24);
        l.sync_queue(&[]);
        l.handle_key(key(KeyCode::Up), 0);
        assert_eq!(l.focus, Focus::Composer);
        assert_eq!(l.mode(), Mode::Compose);
    }
}
