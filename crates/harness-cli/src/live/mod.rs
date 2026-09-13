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
//! (so output never butts against the input — see [`DIVIDER_ROWS`]; the output
//! cursor's own blank row is the spacer, see [`SPACER_ROWS`]),
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
//! The module is split by concern, leaving this file with the [`Live`] state,
//! its key handling, and the resize / interactive-tool hand-off:
//!
//! - [`turn`] — turn orchestration: own the terminal, race the agent's future
//!   against the input stream, drain the queue ([`run_prompt`], [`read_idle`]).
//! - [`events`] — render the agent's streamed events (tokens, tool lines,
//!   retries, compaction) into the scroll region, plus the spinner.
//! - [`paint`] — paint the pinned bottom area: queue table, meters, divider,
//!   and the frameless composer box.
//! - [`completion`] — slash-command + argument completion (the model picker).
//! - [`composer`] — the pure line editor and recallable history.
//! - [`keys`] — keystroke classification (key → intent) and the shared line-edit op.
//! - [`layout`] — queue focus navigation and overflow-window planning.
//! - [`text`] — line rendering (windowing, word-wrap, the themed prompt).
//! - [`terminal`] — the raw-mode RAII guard and the input-forwarding thread.
//!
//! [`MessageQueue`]: crate::queue::MessageQueue

mod card;
mod completion;
mod composer;
mod dispatch;
mod events;
mod keys;
mod layout;
mod lease;
mod paint;
mod pinned;
mod region;
mod sink;
mod terminal;
mod text;
mod turn;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod vt_tests;

pub(crate) use card::summarize_result;
pub(crate) use turn::{read_idle, run_prompt, tool_target, Idle};

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossterm::event::{KeyEvent, KeyEventKind};

use crate::fleet_ui::FleetHub;
use crate::markdown::MarkdownStream;
use crate::render::truncate;
use crate::theme::{LiveSpinner, Ui};

use completion::CompletionItem;
use composer::{Composer, History};
use keys::{apply_buf, classify_key, KeyAction, KeyIntent, Mode};
use layout::Focus;
use lease::ScreenSuspension;
use region::RegionWriter;
use sink::Sink;
use terminal::{region_bottom, title_sequence, CrlfWriter, TitleState, BELL};

/// Blank rows kept between the agent's scrolling output and the pinned input
/// area. Zero: every region write ends in a newline, so the output cursor's
/// own row (where the spinner rides during a turn) is always a blank line
/// directly above the pinned area — that row is the breathing room. A spacer
/// on top of it showed as two empty rows under every reply.
const SPACER_ROWS: usize = 0;

/// Rows for the faint divider rule drawn just above the input area (matching the
/// idle prompt's separator).
const DIVIDER_ROWS: usize = 1;

/// Most input lines shown inside the box at once; beyond this it windows around
/// the caret so a long paste can't push the conversation off-screen.
const MAX_INPUT_ROWS: usize = 8;

/// Display previews are capped to this many characters when snapshotting the
/// queue, then windowed to the terminal width at paint time.
const PREVIEW_CAP: usize = 256;

/// How long after the last inserted character a key-event-burst file drop is
/// considered settled — the deferred media-path check runs then. Burst chars
/// arrive back-to-back (they're already buffered), so anything past this gap
/// is the user typing, not a drop mid-delivery.
const MEDIA_SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// The braille spinner shown on the meter line (and in the terminal title)
/// while a turn runs — one frame per [`SPIN_MS`].
const BRAILLE: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// How long each [`BRAILLE`] frame is held.
const SPIN_MS: u128 = 110;

/// The braille frame for an elapsed duration — a pure function of the clock, so
/// the spinner animates without any per-tick state to keep in sync.
fn braille_frame(elapsed: std::time::Duration) -> char {
    BRAILLE[(elapsed.as_millis() / SPIN_MS) as usize % BRAILLE.len()]
}

/// All mutable state shared between the turn callback and the event loop: the
/// streamed-output renderer, the spinner, the composer, and the navigable queue
/// list.
///
/// The [`MessageQueue`](crate::queue::MessageQueue) itself stays owned by the
/// event loop (the single source of truth); `previews` is only a render
/// snapshot of it, refreshed on every change so the streaming callback can
/// repaint the list without borrowing the queue. Inline editing always reloads
/// the full item text from the queue, so the truncated previews are never
/// edited.
struct Live {
    ui: Ui,
    cols: u16,
    rows: u16,
    out: Sink,
    /// Region writes with the tracked tail line (the spinner) — see [`region`].
    region: RegionWriter,
    md: Option<MarkdownStream<CrlfWriter>>,
    /// The spinner's animation state; its on-screen line is the region tail,
    /// pushed via [`Live::sync_tail`].
    spinner: Option<LiveSpinner>,
    /// `Some` while an interactive tool (the picker) owns the screen: input
    /// forwarding is paused and drawing stops. One owned value — the flags it
    /// bundles can't desync, and its Drop restores the terminal if the turn
    /// dies mid-hand-off. See [`lease`].
    suspension: Option<ScreenSuspension>,
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
    /// The pending repaint level — handlers request, the loops flush (see
    /// [`paint::Repaint`]).
    repaint: paint::Repaint,
    /// Set when a `web_search` call failed for a missing Brave API key, so the
    /// caller can prompt for one once the composer hands back to cooked mode.
    needs_brave_key: bool,
    /// The context-usage trailer (`🧭 context …` + `📊 … used`), pinned just
    /// above the divider rather than printed into the scrollback — so it always
    /// sits right above the input area with the blank spacer separating it from
    /// the last message. Two lines: the context-window fill, then the session's
    /// cumulative token/cost totals. Empty when there's nothing to show.
    status_lines: Vec<String>,
    /// The active model, used to rebuild [`Live::status_lines`] from mid-turn
    /// `Usage` events (which carry token counts but not the model name) so the
    /// pinned meter's tokens and price update live as the agent works.
    model: String,
    /// The session's context window, paired with [`Live::model`] to rebuild the
    /// trailer's `used / window (pct%)` from mid-turn `Usage` events (which
    /// carry the current fill but not the fixed window size).
    context_window: usize,
    /// The compression-savings line (`⊙ compression …`), pinned directly above
    /// [`Live::status_lines`]. Updated in place on every `Compression` event
    /// instead of scrolling a new line into the conversation.
    compression_line: Option<String>,
    /// Slash-command / argument completion for the current composer text,
    /// shown as a picker above the box. Refreshed on every compose edit;
    /// `None` when there's nothing to suggest. One value bundles the
    /// candidates with the highlight/cycle flags so they can't drift apart
    /// (see [`completion::CompletionState`]).
    completion: Option<completion::CompletionState>,
    /// Lazily-loaded model candidates (cloud catalog + installed local) for
    /// `/model` argument completion, cached so we don't rescan on every keystroke.
    model_items: Option<Vec<CompletionItem>>,
    /// The shared fleet hub: while a `spawn_agents` fleet runs mid-turn, its
    /// lanes paint as a pinned block above the meters, and alt+digits switch
    /// which lane's output is being watched.
    fleet: Arc<FleetHub>,
    /// Advances the fleet block's spinner glyphs on the turn ticker.
    fleet_frame: usize,
    /// When a deferred media-path check is due (see
    /// [`Live::tick_media_check`]): set on every non-delimiter insert so a
    /// key-event-burst drop only collapses to a chip once input settles —
    /// collapsing per keystroke would chip a strict prefix of the dropped path
    /// (e.g. `photo.png` inside `photo.png\ copy.png`) and attach the wrong
    /// file.
    media_check: Option<std::time::Instant>,
    /// When the running turn started, or `None` at idle. Drives the meter
    /// line's braille spinner + whole-second timer (see
    /// [`Live::turn_indicator`]) and the `🐂 ⠋ …` terminal title.
    turn_started: Option<std::time::Instant>,
    /// The last terminal title written, so the ~110ms ticker only emits an OSC
    /// sequence when the title actually changes (once a second, not nine times).
    title: Option<String>,
    /// The running command whose output streams into the pinned card (see
    /// [`card`]); `None` between commands.
    tool_card: Option<card::ToolCard>,
    /// The last few sealed tool results, newest last, for Ctrl+O.
    results: std::collections::VecDeque<card::KeptResult>,
    /// What the region's last write was, so the renderer can keep exactly one
    /// blank row between a run of streamed text and the tool/notice lines
    /// around it (see [`events::LastWrite`]).
    last_write: events::LastWrite,
}

impl Live {
    fn new(ui: Ui, cols: u16, rows: u16) -> Self {
        Self::with_sink(ui, cols, rows, Sink::stdout())
    }

    fn with_sink(ui: Ui, cols: u16, rows: u16, out: Sink) -> Self {
        Self {
            ui,
            cols,
            rows,
            region: RegionWriter::new(out.clone()),
            out,
            md: None,
            spinner: None,
            suspension: None,
            composer: Composer::new(),
            history: History::default(),
            focus: Focus::Composer,
            edit: None,
            previews: Vec::new(),
            region_bottom: region_bottom(rows),
            repaint: paint::Repaint::default(),
            needs_brave_key: false,
            status_lines: Vec::new(),
            model: String::new(),
            context_window: 0,
            compression_line: None,
            completion: None,
            model_items: None,
            fleet: FleetHub::global(),
            fleet_frame: 0,
            media_check: None,
            turn_started: None,
            title: None,
            tool_card: None,
            results: std::collections::VecDeque::new(),
            last_write: events::LastWrite::Blank,
        }
    }

    // --- terminal title + bell ---------------------------------------------

    /// Write the terminal title (OSC 0) for `state`, unless the terminal can't
    /// take it (not a TTY, `NO_COLOR`, `TERM=dumb`) or it already says this.
    /// The title is how a backgrounded session announces itself in the tab
    /// bar: `🐂 > repo` idle, `🐂 ⠋ repo` working, `🐂 ! repo` waiting on you.
    pub(super) fn set_title(&mut self, state: TitleState) {
        if !self.ui.decorates() {
            return;
        }
        let text = terminal::window_title(state, &terminal::workspace_name());
        if self.title.as_deref() == Some(text.as_str()) {
            return;
        }
        let _ = write!(self.out, "{}", title_sequence(&text));
        let _ = self.out.flush();
        self.title = Some(text);
    }

    /// Re-title for the turn's current phase: working (with the live spinner
    /// glyph) while a turn runs, idle otherwise. Cheap to call on the ticker.
    pub(super) fn refresh_title(&mut self) {
        match self.turn_started {
            Some(started) => self.set_title(TitleState::Working(braille_frame(started.elapsed()))),
            None => self.set_title(TitleState::Idle),
        }
    }

    /// Ring the terminal bell — for a turn that just finished, or a prompt that
    /// just started waiting on the user. Never for an interrupted turn: the
    /// user is already looking at the screen they just interrupted.
    pub(super) fn bell(&mut self) {
        if !self.ui.decorates() {
            return;
        }
        let _ = write!(self.out, "{BELL}");
        let _ = self.out.flush();
    }

    // --- turn clock ---------------------------------------------------------

    /// Start the meter line's elapsed clock (and the working title).
    pub(super) fn begin_elapsed(&mut self) {
        self.turn_started = Some(std::time::Instant::now());
        self.refresh_title();
    }

    /// Stop the clock — the meter line drops its spinner/timer and the title
    /// goes back to idle.
    pub(super) fn end_elapsed(&mut self) {
        self.turn_started = None;
        self.refresh_title();
    }

    /// The fleet block for the pinned area (empty when no fleet is running).
    /// The pinned rows of the running command's output card.
    fn tool_card_lines(&self) -> Vec<String> {
        match &self.tool_card {
            Some(card) => card.lines(&self.ui, self.cols as usize),
            None => Vec::new(),
        }
    }

    /// Ctrl+O: print the newest sealed tool result in full into the
    /// conversation, under the summary line that stood in for it.
    pub(super) fn expand_last_result(&mut self) {
        let ui = self.ui.clone();
        let block = match self.results.back() {
            Some(kept) => card::expanded_block(&ui, kept, self.cols as usize),
            None => vec![format!("  {}", ui.dim("nothing to expand yet"))],
        };
        self.region.write(&format!("{}\n", block.join("\n")));
        self.request_paint();
    }

    fn fleet_lines(&self) -> Vec<String> {
        let guard = self.fleet.lock();
        match guard.as_ref() {
            Some(state) => {
                crate::fleet_ui::pinned_lines(&self.ui, state, self.cols as usize, self.fleet_frame)
            }
            None => Vec::new(),
        }
    }

    /// Advance the fleet block's animation and report whether the pinned area
    /// needs a repaint this tick — only while a lane is actually running (its
    /// spinner/clock is moving). A fleet whose lanes have all settled leaves
    /// the block static, so the composer stops rewriting it 9×/second for a
    /// picture that no longer changes; a fresh lane event repaints on its own.
    pub(super) fn tick_fleet(&mut self) -> bool {
        let animating = self
            .fleet
            .lock()
            .as_ref()
            .is_some_and(|s| s.has_running_lane());
        if animating {
            self.fleet_frame = self.fleet_frame.wrapping_add(1);
        }
        animating
    }

    /// Fleet lane switching through the shared reducer ([`crate::fleet_ui::
    /// apply_fleet_key`], with the `Shared` vocabulary): alt+digits watch a
    /// lane, alt+0 the overview — bare digits keep typing into the composer.
    /// Only consumes the key while a fleet is actually running.
    fn handle_fleet_key(&mut self, key: &KeyEvent) -> bool {
        let mut guard = self.fleet.lock();
        let Some(state) = guard.as_mut() else {
            return false;
        };
        crate::fleet_ui::apply_fleet_key(
            state,
            key.code,
            key.modifiers,
            crate::fleet_ui::FleetKeys::Shared,
        )
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

    // --- key handling -------------------------------------------------------

    /// Translate a keystroke into a [`KeyAction`], mutating the composer / focus
    /// / inline-edit buffer in place. Queue mutations are deferred to the loop.
    fn handle_key(&mut self, key: KeyEvent, queue_len: usize) -> KeyAction {
        // Windows reports key releases too; act only on presses.
        if key.kind != KeyEventKind::Press {
            return KeyAction::None;
        }
        // Fleet lane switching (alt+digits) outranks composing — but only
        // while a fleet is actually on screen.
        if self.handle_fleet_key(&key) {
            return KeyAction::Redraw;
        }
        match classify_key(
            key.code,
            key.modifiers,
            self.mode(),
            self.composer.is_empty(),
        ) {
            KeyIntent::Ignore => KeyAction::None,
            KeyIntent::Interrupt => KeyAction::Interrupt,
            KeyIntent::CancelTurn => KeyAction::CancelTurn,
            KeyIntent::Exit => KeyAction::Exit,
            KeyIntent::QueueFollowUp => {
                self.accept_completion_on_submit();
                let text = self.composer.take();
                self.history.push(&text);
                self.completion = None;
                KeyAction::QueueFollowUp(text)
            }
            // Only from an empty composer: pulling an item back while a draft
            // is open would silently discard what's typed.
            KeyIntent::PullQueued if self.composer.is_empty() => KeyAction::PullQueued,
            KeyIntent::PullQueued => KeyAction::None,
            KeyIntent::ExpandLast => KeyAction::ExpandLast,
            KeyIntent::ExternalEditor => KeyAction::ExternalEditor,
            KeyIntent::Compose(op) => {
                let inserted = match op {
                    keys::BufOp::Insert(c) => Some(c),
                    _ => None,
                };
                apply_buf(&mut self.composer, op);
                // Some terminals deliver drag-and-drop as a burst of ordinary
                // key events instead of one bracketed-paste event. A delimiter
                // proves the path before it is complete — collapse it to a chip
                // now. A non-delimiter char may still be mid-path (`photo.png`
                // is a strict prefix of `photo.png\ copy.png`), so defer that
                // check until the burst settles (see [`Live::tick_media_check`]).
                match inserted {
                    Some(c) if c.is_whitespace() => {
                        self.media_check = None;
                        self.rewrite_composer_media();
                    }
                    Some(_) => {
                        self.media_check = Some(std::time::Instant::now() + MEDIA_SETTLE);
                    }
                    None => {}
                }
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
            KeyIntent::PasteClipboard => self.paste_clipboard(),
            KeyIntent::Complete => {
                if self.complete() {
                    KeyAction::Redraw
                } else {
                    KeyAction::None
                }
            }
            KeyIntent::ComposerSubmit => {
                self.accept_completion_on_submit();
                let text = self.composer.take();
                self.history.push(&text);
                self.completion = None;
                KeyAction::Submit(text)
            }
            KeyIntent::ComposeUp => {
                if self.move_completion(-1) {
                    return KeyAction::Redraw;
                }
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
                if self.move_completion(1) {
                    return KeyAction::Redraw;
                }
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
                self.completion = None;
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

    /// Replace any complete media path (image/PDF/video) currently in the
    /// composer with a staged chip. This is the fallback for terminals that
    /// type a drop as ordinary key events instead of reporting one
    /// bracketed-paste event. Returns whether the composer was rewritten.
    fn rewrite_composer_media(&mut self) -> bool {
        let current = self.composer.text();
        if let Some(rewritten) = crate::media::rewrite_paste(&current) {
            // The delimiter that proved the path was complete is useful while
            // the user keeps typing, so retain one trailing space.
            let trailing_space = current.chars().last().is_some_and(char::is_whitespace);
            self.composer.set_text(&rewritten);
            if trailing_space {
                self.composer.insert_char(' ');
            }
            return true;
        }
        false
    }

    /// When the deferred media-path check should run, if one is pending — the
    /// event loops use this to wake up once burst input has settled.
    fn media_check_due(&self) -> Option<std::time::Instant> {
        self.media_check
    }

    /// Run the deferred media check once its settle window has elapsed: a
    /// key-event-burst drop with no trailing delimiter collapses to a chip
    /// here, ~[`MEDIA_SETTLE`] after its last character. Returns whether the
    /// composer changed (the caller repaints).
    fn tick_media_check(&mut self) -> bool {
        match self.media_check {
            Some(due) if std::time::Instant::now() >= due => {
                self.media_check = None;
                if self.rewrite_composer_media() {
                    self.refresh_completion();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Insert pasted / drag-dropped text at the caret. Bracketed paste delivers
    /// a drop as one block, so a path with escaped spaces lands intact.
    ///
    /// Three shapes, in order: a pasted media path (image/PDF/video) stages the
    /// file and shows up as its `[Image #N]` / `[PDF #N]` / `[Video #N]` chip;
    /// a **bulky** paste (a stack trace, a whole file) collapses to a
    /// `[Paste #N, +240 lines]` chip that expands back at submit; anything else
    /// goes in verbatim — **newlines included**, since the composer is
    /// multi-line and a pasted snippet should keep its shape.
    ///
    /// The one exception is the inline queue-item editor, which is a single
    /// line: pasting there still flattens newlines to spaces.
    fn insert_paste(&mut self, text: &str) {
        let rewritten = crate::media::rewrite_paste(text);
        let text = rewritten.as_deref().unwrap_or(text);
        if let Some(edit) = self.edit.as_mut() {
            for ch in text.chars() {
                edit.insert_char(if ch == '\n' || ch == '\r' { ' ' } else { ch });
            }
            return;
        }
        // A terminal appends a newline when a file is dropped; that one is a
        // delimiter, not a line — keep it as the space it always was, so the
        // next word doesn't run into the path. Newlines *inside* the paste are
        // the paste's own shape and stay.
        let dropped = text.ends_with('\n');
        let body = text.trim_end_matches(['\n', '\r']);
        let staged;
        let body = if composer::is_bulky(body) {
            staged = composer::stage_paste(body);
            staged.as_str()
        } else {
            body
        };
        for ch in body.chars() {
            // A lone `\r` (an old-Mac line ending, or the CR of a CRLF pair
            // whose `\n` already made the line) would move the caret, not add
            // a line.
            if ch == '\r' {
                continue;
            }
            self.composer.insert_char(ch);
        }
        if dropped {
            self.composer.insert_char(' ');
        }
        self.refresh_completion();
    }

    /// Ctrl+V: read the system clipboard ourselves. A copied image (e.g. a
    /// screenshot) can't arrive through bracketed paste, so it's staged to a
    /// temp PNG and inserted as an `[Image #N]` chip; clipboard text falls back
    /// to an ordinary paste for terminals that pass Ctrl+V through.
    fn paste_clipboard(&mut self) -> KeyAction {
        match crate::media::paste_from_clipboard() {
            crate::media::ClipboardPaste::Image(label) => {
                self.insert_paste(&format!("{label} "));
                KeyAction::Redraw
            }
            crate::media::ClipboardPaste::Text(text) => {
                self.insert_paste(&text);
                KeyAction::Redraw
            }
            crate::media::ClipboardPaste::None => KeyAction::None,
        }
    }

    // --- resize + interactive-tool hand-off --------------------------------

    fn handle_resize(&mut self, cols: u16, rows: u16, items: &[String]) {
        self.cols = cols;
        self.rows = rows;
        self.sync_queue(items);
        self.render_forcing_region();
        self.sync_tail();
        self.flush_paint(); // the tail redraw marked the composer dirty
    }

    /// Whether an interactive tool currently owns the screen.
    fn suspended(&self) -> bool {
        self.suspension.is_some()
    }

    /// Hand the terminal to an interactive tool (the picker): end any open
    /// stream state, then let the [`ScreenSuspension`] drop the scroll region,
    /// show the cursor, leave raw mode, and pause input forwarding — one call,
    /// no flags to keep in sync.
    ///
    /// Crucially the cursor is left right after the last line of conversation
    /// before handing off. Resetting the scroll region (`\x1b[r`) homes the
    /// cursor to the top of the screen, so without restoring it the picker
    /// would draw its first frame at the top — out of view from where the user
    /// is looking — and only become visible once a keypress forced a redraw.
    pub(super) fn hand_off_screen(&mut self, paused: &Arc<AtomicBool>) {
        if self.suspension.is_some() {
            return;
        }
        self.finish(); // stop the spinner, flush any open Markdown
                       // The turn has stopped to ask something: say so in the title and ring
                       // once, so an unattended session gets noticed instead of stalling.
        self.set_title(TitleState::Waiting);
        self.bell();
        self.region.set_muted(true);
        self.suspension = Some(ScreenSuspension::begin(
            self.out.clone(),
            paused,
            self.region_bottom,
            self.rows,
        ));
    }

    /// Reclaim the terminal after the interactive tool finishes: restore raw
    /// mode + input forwarding, restart the thinking spinner, and force a full
    /// repaint (the picker drew over the screen; the forced re-carve reserves
    /// the pinned rows under wherever it left the cursor).
    pub(super) fn reclaim_screen(&mut self) {
        let Some(lease) = self.suspension.take() else {
            return;
        };
        lease.reclaim();
        self.region.set_muted(false);
        self.refresh_title();
        // The picker reset the scroll region; start from the full-height
        // bottom so the forced repaint below clears every row it reserves.
        self.region_bottom = region_bottom(self.rows);
        self.begin_thinking();
        self.render_forcing_region();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::test_support::{alt, ctrl, key, live};
    use super::*;

    // --- Fleet lane switching (alt+digits act only while a fleet runs) -----

    #[test]
    fn alt_digits_switch_fleet_lanes_only_while_a_fleet_runs() {
        use crate::fleet_ui::{FleetHub, FleetState};

        let mut l = live(80, 24);
        // No fleet: alt+1 falls through to normal key handling (not consumed),
        // and plain typing is never hijacked.
        assert!(!l.handle_fleet_key(&alt(KeyCode::Char('1'))));

        // With a fleet on the hub, alt+digits focus lanes and alt+0 clears —
        // while an unmodified digit stays ordinary composer input.
        let hub = FleetHub::global();
        hub.install(FleetState::new(&["scan".into(), "trace".into()], None));
        assert!(l.handle_fleet_key(&alt(KeyCode::Char('2'))));
        assert_eq!(hub.lock().as_ref().unwrap().focused, Some(1));
        assert!(l.handle_fleet_key(&alt(KeyCode::Char('9')))); // out of range clears
        assert_eq!(hub.lock().as_ref().unwrap().focused, None);
        assert!(l.handle_fleet_key(&alt(KeyCode::Char('1'))));
        assert!(l.handle_fleet_key(&alt(KeyCode::Char('0'))));
        assert_eq!(hub.lock().as_ref().unwrap().focused, None);
        assert!(!l.handle_fleet_key(&key(KeyCode::Char('1'))));
        hub.clear();

        // Cleared hub: back to pass-through.
        assert!(!l.handle_fleet_key(&alt(KeyCode::Char('1'))));
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
        // On a non-empty composer Ctrl-D forward-deletes instead of exiting.
        l.handle_key(key(KeyCode::Char('x')), 0);
        l.handle_key(ctrl(KeyCode::Char('a')), 0); // caret to line start
        assert!(matches!(
            l.handle_key(ctrl(KeyCode::Char('d')), 0),
            KeyAction::Redraw
        ));
        assert!(l.composer.is_empty());
    }

    // --- cancel / queue chords ---------------------------------------------

    #[test]
    fn escape_cancels_the_turn_and_leaves_the_draft_alone() {
        let mut l = live(80, 24);
        for ch in "half a thought".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        assert!(matches!(
            l.handle_key(key(KeyCode::Esc), 0),
            KeyAction::CancelTurn
        ));
        // The whole point: Esc costs you nothing you typed.
        assert_eq!(l.composer.text(), "half a thought");
    }

    #[test]
    fn ctrl_enter_and_ctrl_q_hand_the_line_to_the_queue() {
        for chord in [ctrl(KeyCode::Enter), ctrl(KeyCode::Char('q'))] {
            let mut l = live(80, 24);
            for ch in "run the tests".chars() {
                l.handle_key(key(KeyCode::Char(ch)), 0);
            }
            match l.handle_key(chord, 0) {
                KeyAction::QueueFollowUp(text) => assert_eq!(text, "run the tests"),
                other => panic!("expected a queued follow-up, got {other:?}"),
            }
            // The composer is handed over empty, like a submit.
            assert!(l.composer.is_empty());
            // …and the line is recallable with Up, like any other submission.
            assert_eq!(l.history.prev(""), Some("run the tests".to_string()));
        }
    }

    #[test]
    fn alt_up_pulls_a_queued_item_back_only_when_nothing_is_typed() {
        let mut l = live(80, 24);
        assert!(matches!(
            l.handle_key(alt(KeyCode::Up), 1),
            KeyAction::PullQueued
        ));
        // With a draft open, pulling would silently overwrite it — so it doesn't.
        l.handle_key(key(KeyCode::Char('x')), 1);
        assert!(matches!(l.handle_key(alt(KeyCode::Up), 1), KeyAction::None));
        assert_eq!(l.composer.text(), "x");
    }

    // --- paste --------------------------------------------------------------

    #[test]
    fn a_dropped_paths_trailing_newline_stays_a_separator() {
        let mut l = live(80, 24);
        // A drag-dropped path (with a trailing newline the terminal appends):
        // that newline is the drop's delimiter, not a line the user wants.
        l.insert_paste("/tmp/My\\ Shot.png\n");
        assert_eq!(l.composer.take(), "/tmp/My\\ Shot.png ");
    }

    #[test]
    fn a_multi_line_paste_keeps_its_lines() {
        let mut l = live(80, 24);
        l.insert_paste("fn main() {\r\n    println!(\"hi\");\n}");
        // The composer is multi-line: a pasted snippet keeps its shape (and a
        // CRLF paste doesn't leave stray carriage returns behind).
        assert_eq!(l.composer.text(), "fn main() {\n    println!(\"hi\");\n}");
        assert_eq!(l.composer.line_count(), 3);
    }

    #[test]
    fn a_bulky_paste_collapses_to_a_chip_that_expands_at_submit() {
        let mut l = live(80, 24);
        let dump = (0..40)
            .map(|i| format!("stack frame {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        l.insert_paste(&format!("why?\n{dump}"));
        let text = l.composer.text();
        // One tidy line in the composer, not 41 rows of stack trace…
        assert!(text.contains("[Paste #"), "no chip: {text}");
        assert!(text.contains("+41 lines"), "size missing: {text}");
        assert!(!text.contains("stack frame 7"), "raw paste leaked: {text}");
        // …and the model still gets every line of it at submit.
        let expanded = composer::expand_pastes(&text);
        assert!(expanded.contains("stack frame 39"), "lost text: {expanded}");
        assert!(expanded.starts_with("why?\n"), "lost context: {expanded}");
    }

    #[test]
    fn pasted_image_path_becomes_a_chip_that_resolves_back() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("screen.png");
        std::fs::write(&img, [7, 7, 7]).unwrap();

        let mut l = live(80, 24);
        l.insert_paste(&format!("look at {}\n", img.display()));
        let text = l.composer.take();
        // The raw path is hidden behind an `[Image #N]` chip…
        assert!(!text.contains("screen.png"), "path leaked: {text}");
        assert!(text.starts_with("look at [Image #"), "no chip: {text}");
        // …and the chip resolves back to the file at submit time.
        let (attachments, _) = crate::media::resolve_labels(&text);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "screen.png");
    }

    #[test]
    fn media_drop_delivered_as_key_events_becomes_a_chip_once_settled() {
        let dir = tempfile::tempdir().unwrap();
        for (name, chip) in [
            ("screen shot.png", "[Image #"),
            ("the paper.pdf", "[PDF #"),
            ("demo clip.mov", "[Video #"),
            // macOS screenshot naming: U+202F before "PM", which the terminal
            // leaves unescaped in the drop.
            ("Screenshot 2026-07-17 at 9.55.10\u{202f}PM.png", "[Image #"),
        ] {
            let file = dir.path().join(name);
            std::fs::write(&file, [7, 7, 7]).unwrap();
            let dropped = file.display().to_string().replace(' ', "\\ ");

            let mut l = live(80, 24);
            for ch in dropped.chars() {
                l.handle_key(key(KeyCode::Char(ch)), 0);
            }
            // Mid-burst nothing collapses (a prefix of the path may itself
            // name a file); the deferred check runs once input settles.
            assert!(l.media_check_due().is_some(), "no deferred check pending");
            l.media_check = Some(std::time::Instant::now()); // settle now
            assert!(l.tick_media_check(), "settled check should rewrite");
            let text = l.composer.take();

            assert!(!text.contains("shot"), "path leaked: {text}");
            assert!(text.starts_with(chip), "no {chip} chip: {text}");
            let (attachments, _) = crate::media::resolve_labels(&text);
            assert_eq!(attachments.len(), 1);
            assert_eq!(attachments[0].filename, name);
        }
    }

    #[test]
    fn burst_drop_never_chips_a_strict_prefix_of_the_dropped_path() {
        // `photo.png` exists AND is a strict prefix of the dropped
        // `photo.png\ copy.png` — collapsing per keystroke would chip the
        // prefix mid-burst and attach the wrong file.
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("photo.png");
        std::fs::write(&prefix, [7]).unwrap();
        let full = dir.path().join("photo.png copy.png");
        std::fs::write(&full, [7, 7]).unwrap();
        let dropped = full.display().to_string().replace(' ', "\\ ");

        let mut l = live(80, 24);
        for ch in dropped.chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
            assert!(
                !l.composer.text().contains("[Image #"),
                "chipped mid-burst at {:?}",
                l.composer.text()
            );
        }
        l.media_check = Some(std::time::Instant::now()); // settle now
        assert!(l.tick_media_check());
        let text = l.composer.take();
        let (attachments, _) = crate::media::resolve_labels(&text);
        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].filename, "photo.png copy.png",
            "must attach the full dropped file, not its prefix: {text}"
        );
    }

    #[test]
    fn typing_a_space_after_a_media_path_chips_it_immediately() {
        // A delimiter proves the path is complete — no settle wait needed.
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("shot.png");
        std::fs::write(&img, [7]).unwrap();

        let mut l = live(80, 24);
        for ch in img.display().to_string().chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        l.handle_key(key(KeyCode::Char(' ')), 0);
        let text = l.composer.take();
        assert!(text.starts_with("[Image #"), "no immediate chip: {text}");
        assert!(
            l.media_check_due().is_none(),
            "delimiter should clear the deferral"
        );
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
