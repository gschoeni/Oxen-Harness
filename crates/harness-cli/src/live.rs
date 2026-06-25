//! A live, sticky-bottom composer for the interactive REPL.
//!
//! The classic prompt blocks on `rustyline`'s `readline()` until a turn ends, so
//! the user can't type while the model streams. This module takes over a real
//! terminal for the duration of a turn and pins an input line to the bottom row,
//! letting the user stack follow-up messages onto the [`MessageQueue`] while the
//! agent works. Queued messages drain in order as soon as the turn finishes.
//!
//! How it stays out of the output's way: we put the terminal in raw mode, pin the
//! composer to the bottom row, keep a blank spacer row just above it (so output
//! never butts against the prompt — see [`SPACER_ROWS`]), and set a DECSTBM scroll
//! region over the rows *above* that. All turn output (streamed Markdown, tool
//! lines, the spinner) is written into that region — where it scrolls naturally —
//! through a small adapter that turns `\n` into `\r\n` (mandatory in raw mode).
//! The composer is redrawn on row `H` after every output event and keystroke,
//! bracketed by save/restore-cursor so the output position is never disturbed.
//!
//! This path is only ever entered for an interactive TTY (gated by the caller).
//! Everything here is best-effort and always restored on drop.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use harness_agent::{Agent, AgentError, AgentEvent};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::markdown::MarkdownStream;
use crate::queue::MessageQueue;
use crate::render::truncate;
use crate::theme::{LiveSpinner, Ui};

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

    let mut next = Some(first_prompt.to_string());
    let mut exit = false;
    while let Some(prompt) = next.take() {
        match run_one_turn(agent, &prompt, &state, &mut rx, queue, &paused).await {
            TurnOutcome::Done(result) => {
                match result {
                    Ok(_) => state
                        .borrow_mut()
                        .print_line(&crate::context_usage_line(agent, ui)),
                    Err(e) => {
                        let mut s = state.borrow_mut();
                        s.print_line(&format!("  {}", ui.red(&ui.death())));
                        s.print_line(&format!(
                            "  {}",
                            ui.dim(&format!("The trail guide says: {e}"))
                        ));
                    }
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

// ===========================================================================
// The owned terminal (RAII)
// ===========================================================================

/// Owns the terminal for the lifetime of a live session: raw mode, a hidden
/// cursor, and a scroll region over every row but the last. Drop restores
/// everything, no matter how the turn ended.
struct LiveTerminal {
    cols: u16,
    rows: u16,
}

impl LiveTerminal {
    fn new() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let mut out = io::stdout();
        let bottom = region_bottom(rows);
        // Hide the cursor, enable bracketed paste (so a drag-dropped path arrives
        // as one atomic `Event::Paste` instead of a fragile burst of keystrokes),
        // carve a scroll region over rows 1..=H-1, and park the output cursor at
        // the bottom of that region so output scrolls upward.
        let _ = write!(out, "\x1b[?25l\x1b[?2004h\x1b[1;{bottom}r\x1b[{bottom};1H");
        let _ = out.flush();
        Ok(Self { cols, rows })
    }
}

impl Drop for LiveTerminal {
    fn drop(&mut self) {
        let mut out = io::stdout();
        // Disable bracketed paste, reset the scroll region, clear the composer
        // row, show the cursor, and drop to a fresh line for the next cooked-mode
        // prompt.
        let _ = write!(
            out,
            "\x1b[?2004l\x1b[r\x1b[{};1H\x1b[2K\x1b[?25h\r\n",
            self.rows
        );
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// The last row usable for scrolling output (the composer sits below it).
fn region_bottom(rows: u16) -> u16 {
    rows.saturating_sub(1).max(1)
}

// ===========================================================================
// `\n` -> `\r\n` writer for raw mode
// ===========================================================================

/// A `Write` adapter that rewrites bare `\n` as `\r\n`, which raw mode requires
/// to avoid stair-stepped output. `MarkdownStream` writes through this.
struct CrlfWriter {
    out: io::Stdout,
}

impl CrlfWriter {
    fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl Write for CrlfWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut handle = self.out.lock();
        let mut last = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'\n' && (i == 0 || buf[i - 1] != b'\r') {
                handle.write_all(&buf[last..i])?;
                handle.write_all(b"\r\n")?;
                last = i + 1;
            }
        }
        handle.write_all(&buf[last..])?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

// ===========================================================================
// Live render + composer state
// ===========================================================================

/// What a keystroke asks the event loop to do once the pure key handling has
/// already mutated the composer / focus / edit buffer in place. Anything that
/// touches the shared [`MessageQueue`] is deferred to the loop (which owns it);
/// everything self-contained is handled inside `Live` and reported as `Redraw`.
enum KeyAction {
    /// Nothing actionable.
    None,
    /// In-memory state changed; repaint the bottom area.
    Redraw,
    /// Enter in the composer: stack this line onto the queue.
    Submit(String),
    /// Enter/`e` on a focused item: load its text into the inline editor.
    BeginEdit,
    /// Enter while inline-editing: save the edited text back to the queue.
    SaveEdit,
    /// Esc while inline-editing: discard the edit (queue unchanged).
    CancelEdit,
    /// `d`/Delete/Backspace on a focused item: remove it from the queue.
    DeleteFocused,
    /// Ctrl-C — interrupt the turn.
    Interrupt,
    /// Ctrl-D on an empty composer — exit.
    Exit,
}

// ===========================================================================
// Pure queue-navigation logic (no terminal IO — unit-tested in isolation)
// ===========================================================================

/// The most queued rows we ever render at once; beyond this the list windows
/// around the focused item with `…(+k more)` markers.
const MAX_QUEUE_ROWS: usize = 6;

/// Blank rows kept between the agent's scrolling output and the pinned input
/// area, so the prompt always has at least one full line of breathing room.
const SPACER_ROWS: usize = 1;

/// Display previews are capped to this many characters when snapshotting the
/// queue, then windowed to the terminal width at paint time.
const PREVIEW_CAP: usize = 256;

/// Where keyboard focus sits: the bottom composer, or a 0-based queued item.
///
/// The queue is stacked *above* the composer (item 0 on top, the last item just
/// above the composer), so "up" walks toward index 0 and "down" walks back to
/// the composer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Composer,
    Item(usize),
}

impl Focus {
    /// Move focus up. From the composer, enter the list at the item nearest it
    /// (the last one); within the list, step toward the top, clamping at item 0.
    fn up(self, len: usize) -> Focus {
        match self {
            Focus::Composer if len == 0 => Focus::Composer,
            Focus::Composer => Focus::Item(len - 1),
            Focus::Item(i) => Focus::Item(i.min(len.saturating_sub(1)).saturating_sub(1)),
        }
    }

    /// Move focus down. Within the list, step toward the bottom; stepping past
    /// the last item returns to the composer. The composer is the floor.
    fn down(self, len: usize) -> Focus {
        match self {
            Focus::Composer => Focus::Composer,
            Focus::Item(i) => {
                let i = i.min(len.saturating_sub(1));
                if i + 1 >= len {
                    Focus::Composer
                } else {
                    Focus::Item(i + 1)
                }
            }
        }
    }

    /// Re-clamp focus after the queue length changes (e.g. a delete): a focus
    /// past the end snaps to the last item, and an empty queue falls back to the
    /// composer.
    fn clamp(self, len: usize) -> Focus {
        match self {
            Focus::Composer => Focus::Composer,
            Focus::Item(_) if len == 0 => Focus::Composer,
            Focus::Item(i) => Focus::Item(i.min(len - 1)),
        }
    }

    /// The item index the visible window should center on. The composer anchors
    /// on the nearest items (the bottom of the list).
    fn anchor(self, len: usize) -> usize {
        match self {
            Focus::Item(i) => i.min(len.saturating_sub(1)),
            Focus::Composer => len.saturating_sub(1),
        }
    }

    fn item(self) -> Option<usize> {
        match self {
            Focus::Item(i) => Some(i),
            Focus::Composer => None,
        }
    }
}

/// One rendered row of the queue list: either a queued item (by index) or an
/// `…(+k more)` overflow marker standing in for `k` hidden items.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QueueRow {
    Item(usize),
    More(usize),
}

/// Plan the queue rows to display for `len` items with the window centered on
/// `anchor`, showing at most `cap` rows. When the queue overflows `cap`, the
/// top and/or bottom slot becomes an `…(+k more)` marker; the anchored item is
/// always kept visible.
fn plan_rows(len: usize, anchor: usize, cap: usize) -> Vec<QueueRow> {
    if len == 0 || cap == 0 {
        return Vec::new();
    }
    if len <= cap {
        return (0..len).map(QueueRow::Item).collect();
    }

    let mut start = anchor.saturating_sub(cap / 2);
    if start + cap > len {
        start = len - cap;
    }
    let mut rows: Vec<QueueRow> = (start..start + cap).map(QueueRow::Item).collect();

    let above = start;
    let below = len - (start + cap);
    // Replace the edge slots with markers, but never the slot holding the
    // anchor (so the focused item stays on screen).
    if above > 0 && start != anchor {
        rows[0] = QueueRow::More(above + 1);
    }
    let last = cap - 1;
    if below > 0 && start + last != anchor {
        rows[last] = QueueRow::More(below + 1);
    }
    rows
}

/// Plan the queue rows for the current terminal, degrading gracefully on short
/// screens: the list is capped both by [`MAX_QUEUE_ROWS`] and by how many rows
/// are free above the composer (always leaving at least one output row). `frame`
/// is the number of non-list chrome rows the table draws around the items (the
/// header bar + bottom border), reserved so they never push output off-screen.
/// When there's no room, the list collapses to nothing (composer only).
fn queue_rows(len: usize, focus: Focus, rows: u16, cap: usize, frame: usize) -> Vec<QueueRow> {
    let max_list = (rows as usize).saturating_sub(2 + frame);
    let effective = cap.min(max_list);
    if effective == 0 {
        return Vec::new();
    }
    plan_rows(len, focus.anchor(len), effective)
}

/// The number of chrome rows the queue table draws around its items: a header
/// bar (`┌─ N Queued ─┐`) on top and a bottom border (`└─┘`).
const QUEUE_FRAME_ROWS: usize = 2;

/// Which editing surface a keystroke applies to, derived from focus + whether an
/// inline edit is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Typing in the bottom composer.
    Compose,
    /// A queued item is focused (arrow-navigating the list).
    Browse,
    /// Inline-editing the focused item.
    Edit,
}

/// A line-editor operation, shared by the composer and the inline item editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BufOp {
    Insert(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
}

/// The semantic intent of a keystroke, decided purely from the key + current
/// [`Mode`]. Keeping this separate from the IO lets the whole key map be
/// unit-tested without a terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeyIntent {
    Ignore,
    Interrupt,
    Exit,
    Compose(BufOp),
    ComposerSubmit,
    FocusUp,
    FocusDown,
    BeginEdit,
    DeleteItem,
    Edit(BufOp),
    EditCommit,
    EditCancel,
}

/// Map a keystroke to its [`KeyIntent`] for the given mode. Ctrl-C always
/// interrupts; Ctrl-D exits only on an empty composer.
fn classify_key(code: KeyCode, mods: KeyModifiers, mode: Mode, composer_empty: bool) -> KeyIntent {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    if ctrl {
        return match code {
            KeyCode::Char('c') => KeyIntent::Interrupt,
            KeyCode::Char('d') if mode == Mode::Compose && composer_empty => KeyIntent::Exit,
            _ => KeyIntent::Ignore,
        };
    }
    match mode {
        Mode::Compose => match code {
            KeyCode::Enter => KeyIntent::ComposerSubmit,
            KeyCode::Up => KeyIntent::FocusUp,
            KeyCode::Down => KeyIntent::FocusDown,
            KeyCode::Backspace => KeyIntent::Compose(BufOp::Backspace),
            KeyCode::Delete => KeyIntent::Compose(BufOp::Delete),
            KeyCode::Left => KeyIntent::Compose(BufOp::Left),
            KeyCode::Right => KeyIntent::Compose(BufOp::Right),
            KeyCode::Home => KeyIntent::Compose(BufOp::Home),
            KeyCode::End => KeyIntent::Compose(BufOp::End),
            KeyCode::Char(c) if !alt => KeyIntent::Compose(BufOp::Insert(c)),
            _ => KeyIntent::Ignore,
        },
        Mode::Browse => match code {
            KeyCode::Up => KeyIntent::FocusUp,
            KeyCode::Down => KeyIntent::FocusDown,
            KeyCode::Enter | KeyCode::Char('e') => KeyIntent::BeginEdit,
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => KeyIntent::DeleteItem,
            _ => KeyIntent::Ignore,
        },
        Mode::Edit => match code {
            KeyCode::Enter => KeyIntent::EditCommit,
            KeyCode::Esc => KeyIntent::EditCancel,
            KeyCode::Backspace => KeyIntent::Edit(BufOp::Backspace),
            KeyCode::Delete => KeyIntent::Edit(BufOp::Delete),
            KeyCode::Left => KeyIntent::Edit(BufOp::Left),
            KeyCode::Right => KeyIntent::Edit(BufOp::Right),
            KeyCode::Home => KeyIntent::Edit(BufOp::Home),
            KeyCode::End => KeyIntent::Edit(BufOp::End),
            KeyCode::Char(c) if !alt => KeyIntent::Edit(BufOp::Insert(c)),
            _ => KeyIntent::Ignore,
        },
    }
}

/// Apply a [`BufOp`] to a line editor — the one place composer and inline-edit
/// keystrokes converge.
fn apply_buf(c: &mut Composer, op: BufOp) {
    match op {
        BufOp::Insert(ch) => c.insert_char(ch),
        BufOp::Backspace => c.backspace(),
        BufOp::Delete => c.delete(),
        BufOp::Left => c.move_left(),
        BufOp::Right => c.move_right(),
        BufOp::Home => c.move_home(),
        BufOp::End => c.move_end(),
    }
}

/// Render a line editor's text windowed to `avail` columns, optionally drawing a
/// reverse-video caret at the insertion point. Returns the painted body together
/// with its *visible* width (ANSI escapes excluded) so callers that frame it can
/// pad to a fixed cell. Shared by the bottom composer and the inline item editor.
fn render_buffer(c: &Composer, avail: usize, caret: bool) -> (String, usize) {
    let chars = c.chars();
    let cursor = c.cursor();
    // Window the buffer so the caret stays visible on long lines.
    let start = cursor.saturating_sub(avail);
    let end = (start + avail).min(chars.len());
    let mut body = String::new();
    let mut width = 0;
    for (i, ch) in chars.iter().enumerate().take(end).skip(start) {
        if caret && i == cursor {
            body.push_str("\x1b[7m");
            body.push(*ch);
            body.push_str("\x1b[0m");
        } else {
            body.push(*ch);
        }
        width += 1;
    }
    if caret && cursor >= chars.len() {
        body.push_str("\x1b[7m \x1b[0m");
        width += 1;
    }
    (body, width)
}

/// The composer prompt as `(plain, styled)`: the plain form measures width, the
/// styled form is what's drawn. Shows the queue depth when non-empty.
fn composer_prompt(ui: &Ui, depth: usize) -> (String, String) {
    if depth > 0 {
        (
            format!("[{depth} queued] ❯ "),
            format!(
                "{} {} ",
                ui.brown(&format!("[{depth} queued]")),
                ui.accent("❯"),
            ),
        )
    } else {
        ("❯ ".to_string(), format!("{} ", ui.accent("❯")))
    }
}

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
            focus: Focus::Composer,
            edit: None,
            previews: Vec::new(),
            region_bottom: region_bottom(rows),
            needs_brave_key: false,
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

    fn begin_working(&mut self, tool: &str) {
        self.stop_spinner();
        self.spinner = LiveSpinner::new(&self.ui, self.ui.tool_verbs(tool));
        self.draw_spinner();
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
            AgentEvent::Token(t) => {
                if self.md.is_none() {
                    self.stop_spinner();
                    self.write_region("\n");
                    self.md = Some(MarkdownStream::new(self.ui.clone(), CrlfWriter::new()));
                }
                if let Some(md) = self.md.as_mut() {
                    md.push(t);
                }
                self.render_composer();
            }
            // The model started writing a canvas; surface it while its content
            // streams in (the full preview prints on ToolStart).
            AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => {
                self.stop_spinner();
                self.end_markdown();
                self.write_region(&format!(
                    "  {} {}\n",
                    self.ui.green("📄"),
                    self.ui.dim("writing canvas…")
                ));
                self.begin_working(name);
                self.render_composer();
            }
            AgentEvent::ToolPending { .. } => {}
            AgentEvent::ToolStart { name, arguments } => {
                self.stop_spinner();
                self.end_markdown();
                // The picker draws its own UI and reads keys, so hand the screen
                // over to it instead of printing a tool line + spinner.
                if name == harness_tools::ASK_USER_TOOL {
                    self.suspend(paused);
                    return;
                }
                // For a canvas, preview the document inline; the result line then
                // reports the saved path / browser open.
                if name == harness_tools::CANVAS_TOOL {
                    if let Some(block) = crate::canvas::render_canvas_block(&self.ui, arguments) {
                        self.write_region(&format!("{}\n", block.join("\n")));
                    }
                    self.begin_working(name);
                    self.render_composer();
                    return;
                }
                // For file writes/edits, show a colored diff instead of the
                // generic one-line tool preview.
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
                self.begin_working(name);
                self.render_composer();
            }
            AgentEvent::ToolEnd { name, result } => {
                self.stop_spinner();
                if name == harness_tools::ASK_USER_TOOL {
                    self.resume(paused);
                    self.begin_thinking();
                    // Repaint the full list: the picker drew over the screen.
                    self.render_forcing_region();
                    return;
                }
                // Web search with no key: flag it for a prompt once the composer
                // hands back to cooked mode, and show a friendlier line.
                if name == "web_search" && result.contains(harness_tools::web::WEB_SEARCH_NO_KEY) {
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
        }
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

    /// Repaint only the composer row. Used during streaming, where the queue
    /// list is unchanged and protected from scrolling output by the DECSTBM
    /// region, so only the composer needs refreshing.
    fn render_composer(&mut self) {
        if self.suspended {
            return;
        }
        let line = self.composer_line();
        let _ = write!(self.out, "\x1b7\x1b[{};1H\x1b[2K{line}\x1b8", self.rows);
        let _ = self.out.flush();
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
        // One blank row between the agent's output (the scroll region) and the
        // pinned input area below, so the prompt always has breathing room.
        let reserved = (plan.len() + chrome + SPACER_ROWS + 1) as u16;
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
        if !plan.is_empty() {
            let box_w = self.queue_box_w();
            let mut r = new_bottom + 1 + SPACER_ROWS as u16;
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
        buf.push_str(&format!(
            "\x1b[{};1H\x1b[2K{}",
            self.rows,
            self.composer_line()
        ));
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

    /// The table's bottom border: `└────────┘`, spanning the same width as the
    /// header and framed rows.
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

    /// Build the styled composer line, windowed to the terminal width. The caret
    /// shows only when the composer holds focus (not while browsing/editing).
    fn composer_line(&self) -> String {
        let depth = self.previews.len();
        let (plain_prompt, styled_prompt) = composer_prompt(&self.ui, depth);
        let prompt_w = plain_prompt.chars().count();
        let avail = (self.cols as usize)
            .saturating_sub(prompt_w)
            .saturating_sub(1)
            .max(8);
        let caret = matches!(self.focus, Focus::Composer) && self.edit.is_none();
        let (body, _) = render_buffer(&self.composer, avail, caret);
        format!("{styled_prompt}{body}")
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
                KeyAction::Redraw
            }
            KeyIntent::ComposerSubmit => KeyAction::Submit(self.composer.take()),
            KeyIntent::FocusUp => {
                self.focus = self.focus.up(queue_len);
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
        let _ = write!(self.out, "{}", suspend_sequence(self.rows));
        let _ = self.out.flush();
        let _ = terminal::disable_raw_mode();
    }

    /// Reclaim the terminal after the interactive tool finishes.
    fn resume(&mut self, paused: &Arc<AtomicBool>) {
        let _ = terminal::enable_raw_mode();
        self.suspended = false;
        paused.store(false, Ordering::Relaxed);
        let _ = write!(self.out, "{}", resume_sequence(self.rows));
        let _ = self.out.flush();
        // `resume_sequence` carved a composer-only region; record it so the
        // follow-up full repaint re-carves to fit the queue list.
        self.region_bottom = region_bottom(self.rows);
    }
}

/// Escape sequence that hands a clean screen to an interactive tool: reset the
/// scroll region, then clear the composer row and drop onto a fresh line at the
/// bottom (with the cursor shown) so the tool's first frame is fully visible
/// immediately, with no keypress required.
fn suspend_sequence(rows: u16) -> String {
    // `\x1b[r` resets (and homes) the cursor, so we must reposition explicitly:
    // jump to the composer row, clear it, then newline onto a blank bottom line.
    format!("\x1b[r\x1b[{rows};1H\x1b[2K\r\n\x1b[?25h")
}

/// Escape sequence that re-establishes the live layout after an interactive tool
/// finishes: hide the cursor, re-carve the scroll region, and park the output
/// cursor at the bottom of it.
fn resume_sequence(rows: u16) -> String {
    let bottom = region_bottom(rows);
    format!("\x1b[?25l\x1b[1;{bottom}r\x1b[{bottom};1H")
}

// ===========================================================================
// Input thread
// ===========================================================================

/// Spawn a thread that polls for terminal events and forwards them. It never
/// writes to the terminal. `stop` ends it; while `paused`, it yields the event
/// stream (so an interactive tool can read keys itself).
fn spawn_input(
    stop: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
) -> (UnboundedReceiver<Event>, JoinHandle<()>) {
    let (tx, rx): (UnboundedSender<Event>, UnboundedReceiver<Event>) = mpsc::unbounded_channel();
    let stop = stop.clone();
    let paused = paused.clone();
    let handle = thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if paused.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(ev @ Event::Key(_))
                    | Ok(ev @ Event::Resize(_, _))
                    | Ok(ev @ Event::Paste(_)) => {
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    _ => {}
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
    (rx, handle)
}

// ===========================================================================
// Composer — a pure, testable line editor
// ===========================================================================

/// The composer's edit buffer: a line of text and a caret position. Pure and
/// terminal-free so the editing rules can be unit-tested in isolation. The caret
/// is a character index in `0..=buf.len()`.
struct Composer {
    buf: Vec<char>,
    cursor: usize,
}

impl Composer {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
        }
    }

    /// A buffer pre-loaded with `text`, caret at the end — used to seed inline
    /// editing of an existing queued message.
    fn seeded(text: &str) -> Self {
        let buf: Vec<char> = text.chars().collect();
        let cursor = buf.len();
        Self { buf, cursor }
    }

    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The buffer's contents as a string.
    fn text(&self) -> String {
        self.buf.iter().collect()
    }

    fn chars(&self) -> &[char] {
        &self.buf
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    /// Insert `c` at the caret and advance past it.
    fn insert_char(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Delete the character before the caret (Backspace).
    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buf.remove(self.cursor);
        }
    }

    /// Delete the character at the caret (Delete / forward-delete).
    fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.buf.len();
    }

    /// Take the current line, clearing the buffer and resetting the caret.
    fn take(&mut self) -> String {
        let line: String = self.buf.drain(..).collect();
        self.cursor = 0;
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_sequence_clears_a_clean_bottom_line_for_the_picker() {
        let seq = suspend_sequence(24);
        // The scroll region must be reset before anything else so the picker
        // isn't confined to the old region.
        assert!(
            seq.starts_with("\x1b[r"),
            "region reset must come first: {seq:?}"
        );
        // It repositions to the composer row and clears it (resetting the region
        // homes the cursor to the top, so an explicit move is required).
        assert!(
            seq.contains("\x1b[24;1H"),
            "must move to the bottom row: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[2K"),
            "must clear the composer row: {seq:?}"
        );
        // And the cursor is shown so the picker draws on a visible line.
        assert!(seq.contains("\x1b[?25h"), "must show the cursor: {seq:?}");
        // The region reset precedes the absolute cursor move.
        assert!(seq.find("\x1b[r").unwrap() < seq.find("\x1b[24;1H").unwrap());
    }

    #[test]
    fn resume_sequence_reestablishes_the_live_layout() {
        let seq = resume_sequence(24);
        assert!(
            seq.contains("\x1b[?25l"),
            "must hide the cursor again: {seq:?}"
        );
        // Region carved over every row but the composer row (24 -> bottom 23).
        assert!(
            seq.contains("\x1b[1;23r"),
            "must re-carve the scroll region: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[23;1H"),
            "must park at the region bottom: {seq:?}"
        );
    }

    #[test]
    fn suspend_sequence_handles_a_tiny_terminal() {
        // A 1-row terminal must not underflow when computing the region bottom.
        let _ = suspend_sequence(1);
        assert!(resume_sequence(1).contains("\x1b[1;1r"));
    }

    fn typed(text: &str) -> Composer {
        let mut c = Composer::new();
        for ch in text.chars() {
            c.insert_char(ch);
        }
        c
    }

    #[test]
    fn insert_appends_and_advances_caret() {
        let c = typed("hi");
        assert_eq!(c.chars().iter().collect::<String>(), "hi");
        assert_eq!(c.cursor(), 2);
        assert!(!c.is_empty());
    }

    #[test]
    fn insert_respects_caret_position() {
        let mut c = typed("ac");
        c.move_left(); // between a and c
        c.insert_char('b');
        assert_eq!(c.chars().iter().collect::<String>(), "abc");
        assert_eq!(c.cursor(), 2);
    }

    #[test]
    fn backspace_deletes_before_caret_and_guards_start() {
        let mut c = typed("ab");
        c.backspace();
        assert_eq!(c.chars().iter().collect::<String>(), "a");
        c.backspace();
        assert!(c.is_empty());
        c.backspace(); // no-op at start
        assert!(c.is_empty());
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn delete_removes_at_caret_and_guards_end() {
        let mut c = typed("abc");
        c.move_home();
        c.delete();
        assert_eq!(c.chars().iter().collect::<String>(), "bc");
        assert_eq!(c.cursor(), 0);
        c.move_end();
        c.delete(); // no-op at end
        assert_eq!(c.chars().iter().collect::<String>(), "bc");
    }

    #[test]
    fn cursor_movement_clamps_at_both_edges() {
        let mut c = typed("ab");
        c.move_end();
        assert_eq!(c.cursor(), 2);
        c.move_right(); // clamp at end
        assert_eq!(c.cursor(), 2);
        c.move_home();
        assert_eq!(c.cursor(), 0);
        c.move_left(); // clamp at start
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn take_returns_line_and_clears() {
        let mut c = typed("send me");
        assert_eq!(c.take(), "send me");
        assert!(c.is_empty());
        assert_eq!(c.cursor(), 0);
        // A fresh take after clearing yields an empty line.
        assert_eq!(c.take(), "");
    }

    #[test]
    fn editing_handles_unicode_by_char_not_byte() {
        let mut c = typed("café");
        assert_eq!(c.cursor(), 4);
        c.backspace();
        assert_eq!(c.chars().iter().collect::<String>(), "caf");
        c.insert_char('é');
        assert_eq!(c.take(), "café");
    }

    #[test]
    fn seeded_composer_loads_text_with_caret_at_end() {
        let mut c = Composer::seeded("hello");
        assert_eq!(c.cursor(), 5);
        c.backspace();
        assert_eq!(c.take(), "hell");
        // Seeding an empty string is a no-op editor.
        assert!(Composer::seeded("").is_empty());
    }

    // --- focus state machine -----------------------------------------------

    #[test]
    fn focus_up_enters_list_at_nearest_item_then_clamps_at_top() {
        assert_eq!(Focus::Composer.up(3), Focus::Item(2));
        assert_eq!(Focus::Item(2).up(3), Focus::Item(1));
        assert_eq!(Focus::Item(0).up(3), Focus::Item(0));
        // An empty queue keeps focus on the composer.
        assert_eq!(Focus::Composer.up(0), Focus::Composer);
    }

    #[test]
    fn focus_down_walks_back_to_the_composer() {
        assert_eq!(Focus::Item(0).down(3), Focus::Item(1));
        assert_eq!(Focus::Item(2).down(3), Focus::Composer);
        assert_eq!(Focus::Composer.down(3), Focus::Composer);
    }

    #[test]
    fn focus_clamp_snaps_into_range_after_a_change() {
        assert_eq!(Focus::Item(5).clamp(3), Focus::Item(2));
        assert_eq!(Focus::Item(0).clamp(0), Focus::Composer);
        assert_eq!(Focus::Composer.clamp(0), Focus::Composer);
    }

    #[test]
    fn focus_anchor_centers_window_on_focus_or_bottom() {
        assert_eq!(Focus::Item(4).anchor(10), 4);
        assert_eq!(Focus::Composer.anchor(10), 9);
        assert_eq!(Focus::Composer.anchor(0), 0);
    }

    // --- visible-window computation ----------------------------------------

    #[test]
    fn plan_rows_shows_everything_within_cap() {
        assert_eq!(
            plan_rows(3, 0, 6),
            vec![QueueRow::Item(0), QueueRow::Item(1), QueueRow::Item(2)]
        );
        assert!(plan_rows(0, 0, 6).is_empty());
    }

    #[test]
    fn plan_rows_windows_with_overflow_markers() {
        // Anchored at the top: only a bottom `…(+k more)` marker.
        assert_eq!(
            plan_rows(10, 0, 6),
            vec![
                QueueRow::Item(0),
                QueueRow::Item(1),
                QueueRow::Item(2),
                QueueRow::Item(3),
                QueueRow::Item(4),
                QueueRow::More(5),
            ]
        );
        // Anchored at the bottom: only a top marker.
        assert_eq!(
            plan_rows(10, 9, 6),
            vec![
                QueueRow::More(5),
                QueueRow::Item(5),
                QueueRow::Item(6),
                QueueRow::Item(7),
                QueueRow::Item(8),
                QueueRow::Item(9),
            ]
        );
        // Anchored in the middle: markers on both ends, focus stays visible.
        let mid = plan_rows(10, 5, 6);
        assert_eq!(mid.len(), 6);
        assert_eq!(mid.first(), Some(&QueueRow::More(3)));
        assert_eq!(mid.last(), Some(&QueueRow::More(3)));
        assert!(mid.contains(&QueueRow::Item(5)));
    }

    #[test]
    fn queue_rows_degrades_on_short_terminals() {
        let frame = QUEUE_FRAME_ROWS;
        // Roomy terminal shows every item.
        assert_eq!(queue_rows(3, Focus::Composer, 24, 6, frame).len(), 3);
        // A short terminal has no room for the framed list (composer only).
        assert!(queue_rows(5, Focus::Composer, 3, 6, frame).is_empty());
        // The row cap is honored even on a tall screen with a long queue.
        assert_eq!(queue_rows(20, Focus::Composer, 50, 6, frame).len(), 6);
        // The header/footer frame is reserved out of the list budget: a 9-row
        // terminal leaves 9 - 2 - 2 = 5 rows for items, not 7.
        assert_eq!(queue_rows(20, Focus::Composer, 9, 6, frame).len(), 5);
    }

    // --- key dispatch ------------------------------------------------------

    #[test]
    fn classify_key_maps_compose_mode() {
        let n = KeyModifiers::NONE;
        assert_eq!(
            classify_key(KeyCode::Enter, n, Mode::Compose, false),
            KeyIntent::ComposerSubmit
        );
        assert_eq!(
            classify_key(KeyCode::Up, n, Mode::Compose, false),
            KeyIntent::FocusUp
        );
        assert_eq!(
            classify_key(KeyCode::Down, n, Mode::Compose, true),
            KeyIntent::FocusDown
        );
        assert_eq!(
            classify_key(KeyCode::Char('a'), n, Mode::Compose, false),
            KeyIntent::Compose(BufOp::Insert('a'))
        );
    }

    #[test]
    fn classify_key_browse_edits_and_deletes() {
        let n = KeyModifiers::NONE;
        assert_eq!(
            classify_key(KeyCode::Enter, n, Mode::Browse, false),
            KeyIntent::BeginEdit
        );
        assert_eq!(
            classify_key(KeyCode::Char('e'), n, Mode::Browse, false),
            KeyIntent::BeginEdit
        );
        for c in [KeyCode::Char('d'), KeyCode::Delete, KeyCode::Backspace] {
            assert_eq!(
                classify_key(c, n, Mode::Browse, false),
                KeyIntent::DeleteItem
            );
        }
        // Stray typing while browsing is ignored.
        assert_eq!(
            classify_key(KeyCode::Char('x'), n, Mode::Browse, false),
            KeyIntent::Ignore
        );
    }

    #[test]
    fn classify_key_edit_mode_commits_and_cancels() {
        let n = KeyModifiers::NONE;
        assert_eq!(
            classify_key(KeyCode::Enter, n, Mode::Edit, false),
            KeyIntent::EditCommit
        );
        assert_eq!(
            classify_key(KeyCode::Esc, n, Mode::Edit, false),
            KeyIntent::EditCancel
        );
        assert_eq!(
            classify_key(KeyCode::Char('z'), n, Mode::Edit, false),
            KeyIntent::Edit(BufOp::Insert('z'))
        );
    }

    #[test]
    fn classify_key_ctrl_c_interrupts_anywhere_ctrl_d_exits_only_on_empty() {
        let c = KeyModifiers::CONTROL;
        for m in [Mode::Compose, Mode::Browse, Mode::Edit] {
            assert_eq!(
                classify_key(KeyCode::Char('c'), c, m, true),
                KeyIntent::Interrupt
            );
        }
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Compose, true),
            KeyIntent::Exit
        );
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Compose, false),
            KeyIntent::Ignore
        );
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Browse, true),
            KeyIntent::Ignore
        );
    }

    // --- Live wiring (no TTY: handle_key + buffer state, never paint) ------

    fn live(cols: u16, rows: u16) -> Live {
        Live::new(
            Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default())),
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

    fn plain_live(cols: u16, rows: u16) -> Live {
        Live::new(
            Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default())),
            cols,
            rows,
        )
    }

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
