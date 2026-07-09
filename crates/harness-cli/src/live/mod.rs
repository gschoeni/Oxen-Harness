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

mod completion;
mod composer;
mod events;
mod keys;
mod layout;
mod paint;
mod terminal;
mod text;
mod turn;

#[cfg(test)]
mod test_support;

pub(crate) use turn::{read_idle, run_prompt, tool_target, Idle};

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::fleet_ui::FleetHub;
use crate::markdown::MarkdownStream;
use crate::render::truncate;
use crate::theme::{LiveSpinner, Ui};

use completion::CompletionItem;
use composer::{Composer, History};
use keys::{apply_buf, classify_key, KeyAction, KeyIntent, Mode};
use layout::Focus;
use terminal::{region_bottom, resume_sequence, suspend_sequence, CrlfWriter};

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
/// description. Kept in sync with [`crate::repl::parse_command`] — a test below
/// fails if an entry here stops being a recognized command.
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/model", "pick, switch, or add a model"),
    ("/theme", "change the theme"),
    ("/queue", "manage the message queue"),
    ("/loop", "run or list loops"),
    (
        "/code-review",
        "review your changes (find → verify → report)",
    ),
    ("/export", "export the transcript"),
    ("/skills", "list the skills on hand"),
    ("/retry", "re-drive a turn that died mid-stream"),
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
    /// text (full-line replacements), shown as a picker above the box. Refreshed
    /// on every compose edit; empty when there's nothing to suggest.
    completion: Vec<CompletionItem>,
    /// The candidate currently selected by Tab cycling (highlights the picker and
    /// drives menu-complete). `None` until the first Tab.
    comp_index: Option<usize>,
    /// Whether the candidates form an argument *picker* (highlighted row, ↑/↓
    /// navigation, Enter runs the selection) rather than plain command-word
    /// completion. Set alongside the candidates in `refresh_completion`.
    comp_picker: bool,
    /// Whether the composer text was set by the last Tab (menu-complete), so the
    /// next Tab advances the cycle. Cleared by edits and arrow selection — a Tab
    /// after arrowing applies the highlighted row even when its replacement
    /// happens to equal the current text (e.g. the `/model ` "add" row).
    comp_applied: bool,
    /// Whether the user explicitly walked the completion list (↑/↓/Tab) since
    /// the last edit. A navigated row is accepted by Enter unconditionally; an
    /// auto-highlighted one only under the rules in
    /// `accept_completion_on_submit`.
    comp_navigated: bool,
    /// Lazily-loaded model candidates (cloud catalog + installed local) for
    /// `/model` argument completion, cached so we don't rescan on every keystroke.
    model_items: Option<Vec<CompletionItem>>,
    /// The shared fleet hub: while a `spawn_agents` fleet runs mid-turn, its
    /// lanes paint as a pinned block above the meters, and alt+digits switch
    /// which lane's output is being watched.
    fleet: Arc<FleetHub>,
    /// Advances the fleet block's spinner glyphs on the turn ticker.
    fleet_frame: usize,
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
            comp_picker: false,
            comp_applied: false,
            comp_navigated: false,
            model_items: None,
            fleet: FleetHub::global(),
            fleet_frame: 0,
        }
    }

    /// The fleet block for the pinned area (empty when no fleet is running).
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

    /// Alt+digit switches which fleet lane is being watched (alt+0 → overview).
    /// Only consumes the key while a fleet is actually running.
    fn handle_fleet_key(&mut self, key: &KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        let KeyCode::Char(c @ '0'..='9') = key.code else {
            return false;
        };
        let mut guard = self.fleet.lock();
        let Some(state) = guard.as_mut() else {
            return false;
        };
        match c {
            '0' => state.focus(None),
            _ => state.focus(Some(c as usize - '1' as usize)),
        }
        true
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
                self.completion.clear();
                self.comp_index = None;
                self.comp_picker = false;
                self.comp_navigated = false;
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
                self.completion.clear();
                self.comp_index = None;
                self.comp_picker = false;
                self.comp_navigated = false;
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
    /// block, so a path with escaped spaces lands intact. A pasted image path
    /// stages the image and shows up as its `[Image N]` chip instead.
    fn insert_paste(&mut self, text: &str) {
        let rewritten = crate::images::rewrite_paste(text);
        let text = rewritten.as_deref().unwrap_or(text);
        let target = self.edit.as_mut().unwrap_or(&mut self.composer);
        for ch in text.chars() {
            let ch = if ch == '\n' || ch == '\r' { ' ' } else { ch };
            target.insert_char(ch);
        }
        if self.edit.is_none() {
            self.refresh_completion();
        }
    }

    /// Ctrl+V: read the system clipboard ourselves. A copied image (e.g. a
    /// screenshot) can't arrive through bracketed paste, so it's staged to a
    /// temp PNG and inserted as an `[Image N]` chip; clipboard text falls back
    /// to an ordinary paste for terminals that pass Ctrl+V through.
    fn paste_clipboard(&mut self) -> KeyAction {
        match crate::images::paste_from_clipboard() {
            crate::images::ClipboardPaste::Image(label) => {
                self.insert_paste(&format!("{label} "));
                KeyAction::Redraw
            }
            crate::images::ClipboardPaste::Text(text) => {
                self.insert_paste(&text);
                KeyAction::Redraw
            }
            crate::images::ClipboardPaste::None => KeyAction::None,
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
    use crossterm::event::KeyCode;

    use super::test_support::{alt, ctrl, key, live};
    use super::*;

    #[test]
    fn every_completion_entry_is_a_recognized_command() {
        use crate::repl::{parse_command, Command};
        for (cmd, _) in SLASH_COMMANDS {
            assert!(
                !matches!(parse_command(cmd), Command::Prompt(_)),
                "`{cmd}` is offered by completion but parse_command doesn't recognize it"
            );
        }
    }

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

    // --- paste --------------------------------------------------------------

    #[test]
    fn paste_inserts_into_the_composer_and_flattens_newlines() {
        let mut l = live(80, 24);
        // A drag-dropped path (with a trailing newline the terminal appends).
        l.insert_paste("/tmp/My\\ Shot.png\n");
        assert_eq!(l.composer.take(), "/tmp/My\\ Shot.png ");
    }

    #[test]
    fn pasted_image_path_becomes_a_chip_that_resolves_back() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("screen.png");
        std::fs::write(&img, [7, 7, 7]).unwrap();

        let mut l = live(80, 24);
        l.insert_paste(&format!("look at {}\n", img.display()));
        let text = l.composer.take();
        // The raw path is hidden behind an `[Image N]` chip…
        assert!(!text.contains("screen.png"), "path leaked: {text}");
        assert!(text.starts_with("look at [Image "), "no chip: {text}");
        // …and the chip resolves back to the file at submit time.
        let (attachments, _) = crate::images::resolve_labels(&text);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "screen.png");
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
