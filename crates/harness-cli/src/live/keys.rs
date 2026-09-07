//! Keystroke handling for the live input box, split into two pure stages so the
//! whole key map can be unit-tested without a terminal: [`classify_key`] decides
//! a keystroke's semantic [`KeyIntent`] from the key + [`Mode`], and [`apply_buf`]
//! applies a [`BufOp`] to a line editor — the one place the composer and the
//! inline item editor converge.

use crossterm::event::{KeyCode, KeyModifiers};

use super::composer::Composer;

/// What a keystroke asks the event loop to do once the pure key handling has
/// already mutated the composer / focus / edit buffer in place. Anything that
/// touches the shared [`crate::queue::MessageQueue`] is deferred to the loop
/// (which owns it); everything self-contained is handled inside `Live` and
/// reported as `Redraw`.
#[derive(Debug)]
pub(super) enum KeyAction {
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
    /// Esc — cancel a running turn without touching the draft or the exit
    /// guard (see [`KeyIntent::CancelTurn`]).
    CancelTurn,
    /// Ctrl+Enter / Ctrl+Q in the composer: stack this line as a follow-up
    /// that runs *after* the turn, instead of steering the running one.
    QueueFollowUp(String),
    /// Alt+Up on an empty composer: pull the newest queued item back out of
    /// the queue and into the composer for another edit.
    PullQueued,
    /// Ctrl+O: print the newest sealed tool result in full.
    ExpandLast,
    /// Ctrl+G: hand the draft to `$EDITOR` (idle only).
    ExternalEditor,
    /// Ctrl-D on an empty composer — exit.
    Exit,
}

/// Which editing surface a keystroke applies to, derived from focus + whether an
/// inline edit is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    /// Typing in the bottom composer.
    Compose,
    /// A queued item is focused (arrow-navigating the list).
    Browse,
    /// Inline-editing the focused item.
    Edit,
}

/// A line-editor operation, shared by the composer and the inline item editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BufOp {
    Insert(char),
    Backspace,
    Delete,
    Left,
    Right,
    /// Jump to the start of the previous word (Alt/Ctrl+Left, Alt+B).
    WordLeft,
    /// Jump past the end of the next word (Alt/Ctrl+Right, Alt+F).
    WordRight,
    Home,
    End,
    /// Delete the previous word (Alt/Ctrl+Backspace, Ctrl+W).
    DeleteWordBack,
    /// Delete the next word (Alt+D, Alt/Ctrl+Delete).
    DeleteWordForward,
    /// Delete to the end of the line (Ctrl+K).
    KillToEnd,
    /// Delete to the start of the line (Ctrl+U).
    KillToStart,
}

/// The semantic intent of a keystroke, decided purely from the key + current
/// [`Mode`]. Keeping this separate from the IO lets the whole key map be
/// unit-tested without a terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum KeyIntent {
    Ignore,
    Interrupt,
    /// Esc: cancel the *turn*, never the draft. Distinct from
    /// [`KeyIntent::Interrupt`] because Ctrl-C also clears the composer and
    /// arms the staged exit — Escape must do neither, so a mis-hit Esc can
    /// never cost you a half-typed message or drop you out of the session.
    CancelTurn,
    /// Ctrl+Enter / Ctrl+Q: queue the composer's text as a follow-up (it runs
    /// after the current turn) instead of steering the running one. Ctrl+Enter
    /// only reaches us on terminals speaking the kitty keyboard protocol, so
    /// Ctrl+Q is the portable spelling of the same thing.
    QueueFollowUp,
    /// Alt+Up: pull the most recent queued item back into the composer.
    PullQueued,
    /// Ctrl+O: print the newest tool result in full.
    ExpandLast,
    /// Ctrl+G: open the draft in `$EDITOR`.
    ExternalEditor,
    Exit,
    Compose(BufOp),
    /// Insert a hard line break in the composer (Alt/Shift+Enter, Ctrl+J).
    ComposeNewline,
    ComposerSubmit,
    /// Ctrl+V: read the system clipboard directly. Bracketed paste can only
    /// deliver text, so this is how a copied screenshot gets in as an image.
    PasteClipboard,
    /// Tab in the composer: menu-complete the slash command / argument.
    Complete,
    /// Up in the composer: move a line up, else recall older history / focus the
    /// queue (resolved against composer + history + queue state in `handle_key`).
    ComposeUp,
    /// Down in the composer: move a line down, else recall newer history.
    ComposeDown,
    FocusUp,
    FocusDown,
    BeginEdit,
    DeleteItem,
    Edit(BufOp),
    EditCommit,
    EditCancel,
}

/// Map a keystroke to its [`KeyIntent`] for the given mode. Ctrl-C always
/// interrupts; Ctrl-D exits only on an empty composer (elsewhere it's the
/// readline forward-delete).
pub(super) fn classify_key(
    code: KeyCode,
    mods: KeyModifiers,
    mode: Mode,
    composer_empty: bool,
) -> KeyIntent {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    if ctrl && code == KeyCode::Char('c') {
        return KeyIntent::Interrupt;
    }
    if ctrl && code == KeyCode::Char('d') && mode == Mode::Compose && composer_empty {
        return KeyIntent::Exit;
    }
    // Ctrl+Q is the portable "queue this as a follow-up" chord (Ctrl+Enter
    // needs the kitty protocol); it works from the composer only.
    if ctrl && code == KeyCode::Char('q') && mode == Mode::Compose {
        return KeyIntent::QueueFollowUp;
    }
    // Ctrl+V pastes from the system clipboard into whichever editor is open
    // (the terminal's own paste arrives as `Event::Paste` instead).
    if ctrl && code == KeyCode::Char('v') && mode != Mode::Browse {
        return KeyIntent::PasteClipboard;
    }
    match mode {
        Mode::Compose => {
            // Compose-specific chords first; everything else is a buffer edit.
            match code {
                // Ctrl-J is a portable "insert newline" that survives terminals
                // which don't distinguish Shift/Alt+Enter.
                KeyCode::Char('j') if ctrl => return KeyIntent::ComposeNewline,
                KeyCode::Char('o') if ctrl && !alt => return KeyIntent::ExpandLast,
                KeyCode::Char('g') if ctrl && !alt => return KeyIntent::ExternalEditor,
                // Alt/Shift+Enter add a line; plain Enter sends (or queues).
                KeyCode::Enter if alt || mods.contains(KeyModifiers::SHIFT) => {
                    return KeyIntent::ComposeNewline
                }
                // Ctrl+Enter stacks a follow-up for after the turn; plain
                // Enter still sends (mid-turn: steers).
                KeyCode::Enter if ctrl => return KeyIntent::QueueFollowUp,
                KeyCode::Enter if !ctrl => return KeyIntent::ComposerSubmit,
                KeyCode::Tab if !ctrl && !alt => return KeyIntent::Complete,
                // Esc cancels the running turn — never the draft (Ctrl-C owns
                // that), so it is inert at idle.
                KeyCode::Esc if !ctrl && !alt => return KeyIntent::CancelTurn,
                // Alt+Up un-queues the newest follow-up back into the composer.
                KeyCode::Up if alt && !ctrl => return KeyIntent::PullQueued,
                KeyCode::Up if !ctrl && !alt => return KeyIntent::ComposeUp,
                KeyCode::Down if !ctrl && !alt => return KeyIntent::ComposeDown,
                _ => {}
            }
            edit_op(code, mods)
                .map(KeyIntent::Compose)
                .unwrap_or(KeyIntent::Ignore)
        }
        Mode::Browse => {
            if ctrl || alt {
                return KeyIntent::Ignore;
            }
            match code {
                KeyCode::Up => KeyIntent::FocusUp,
                KeyCode::Down => KeyIntent::FocusDown,
                KeyCode::Enter | KeyCode::Char('e') => KeyIntent::BeginEdit,
                KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => KeyIntent::DeleteItem,
                _ => KeyIntent::Ignore,
            }
        }
        Mode::Edit => {
            match code {
                KeyCode::Enter if !ctrl && !alt => return KeyIntent::EditCommit,
                KeyCode::Esc => return KeyIntent::EditCancel,
                _ => {}
            }
            edit_op(code, mods)
                .map(KeyIntent::Edit)
                .unwrap_or(KeyIntent::Ignore)
        }
    }
}

/// The line-editing key map shared by the composer and the inline item editor:
/// plain caret movement plus the usual readline/terminal chords — Ctrl+A/E
/// (line start/end), Ctrl/Alt+arrows and Alt+B/F (word hops), Ctrl+W and
/// Alt+Backspace (delete word back), Alt+D (delete word forward), Ctrl+K/U
/// (kill to line end/start), Ctrl+B/F (char moves), Ctrl+D (forward delete).
fn edit_op(code: KeyCode, mods: KeyModifiers) -> Option<BufOp> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    if ctrl {
        return match code {
            KeyCode::Char('a') => Some(BufOp::Home),
            KeyCode::Char('e') => Some(BufOp::End),
            KeyCode::Char('b') => Some(BufOp::Left),
            KeyCode::Char('f') => Some(BufOp::Right),
            KeyCode::Char('d') => Some(BufOp::Delete),
            KeyCode::Char('h') => Some(BufOp::Backspace),
            KeyCode::Char('w') | KeyCode::Backspace => Some(BufOp::DeleteWordBack),
            KeyCode::Char('k') => Some(BufOp::KillToEnd),
            KeyCode::Char('u') => Some(BufOp::KillToStart),
            KeyCode::Left => Some(BufOp::WordLeft),
            KeyCode::Right => Some(BufOp::WordRight),
            KeyCode::Delete => Some(BufOp::DeleteWordForward),
            _ => None,
        };
    }
    if alt {
        // Terminals send Option/Alt combos either as modified keys or as
        // ESC-prefixed chars (e.g. macOS Terminal's option+arrow presets emit
        // ESC B / ESC F); crossterm reports both with the ALT modifier.
        return match code {
            KeyCode::Left | KeyCode::Char('b') | KeyCode::Char('B') => Some(BufOp::WordLeft),
            KeyCode::Right | KeyCode::Char('f') | KeyCode::Char('F') => Some(BufOp::WordRight),
            KeyCode::Backspace => Some(BufOp::DeleteWordBack),
            KeyCode::Delete | KeyCode::Char('d') => Some(BufOp::DeleteWordForward),
            _ => None,
        };
    }
    match code {
        KeyCode::Backspace => Some(BufOp::Backspace),
        KeyCode::Delete => Some(BufOp::Delete),
        KeyCode::Left => Some(BufOp::Left),
        KeyCode::Right => Some(BufOp::Right),
        KeyCode::Home => Some(BufOp::Home),
        KeyCode::End => Some(BufOp::End),
        KeyCode::Char(c) => Some(BufOp::Insert(c)),
        _ => None,
    }
}

/// Apply a [`BufOp`] to a line editor — the one place composer and inline-edit
/// keystrokes converge.
pub(super) fn apply_buf(c: &mut Composer, op: BufOp) {
    match op {
        BufOp::Insert(ch) => c.insert_char(ch),
        BufOp::Backspace => c.backspace(),
        BufOp::Delete => c.delete(),
        BufOp::Left => c.move_left(),
        BufOp::Right => c.move_right(),
        BufOp::WordLeft => c.move_word_left(),
        BufOp::WordRight => c.move_word_right(),
        BufOp::Home => c.move_home(),
        BufOp::End => c.move_end(),
        BufOp::DeleteWordBack => c.delete_word_back(),
        BufOp::DeleteWordForward => c.delete_word_forward(),
        BufOp::KillToEnd => c.kill_to_end(),
        BufOp::KillToStart => c.kill_to_start(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_key_maps_compose_mode() {
        let n = KeyModifiers::NONE;
        assert_eq!(
            classify_key(KeyCode::Enter, n, Mode::Compose, false),
            KeyIntent::ComposerSubmit
        );
        // Shift/Alt+Enter and Ctrl-J insert a newline instead of sending.
        assert_eq!(
            classify_key(KeyCode::Enter, KeyModifiers::SHIFT, Mode::Compose, false),
            KeyIntent::ComposeNewline
        );
        assert_eq!(
            classify_key(KeyCode::Enter, KeyModifiers::ALT, Mode::Compose, false),
            KeyIntent::ComposeNewline
        );
        assert_eq!(
            classify_key(
                KeyCode::Char('j'),
                KeyModifiers::CONTROL,
                Mode::Compose,
                false
            ),
            KeyIntent::ComposeNewline
        );
        // Up/Down are resolved (line move vs history vs queue) in handle_key.
        assert_eq!(
            classify_key(KeyCode::Up, n, Mode::Compose, false),
            KeyIntent::ComposeUp
        );
        assert_eq!(
            classify_key(KeyCode::Down, n, Mode::Compose, true),
            KeyIntent::ComposeDown
        );
        assert_eq!(
            classify_key(KeyCode::Char('a'), n, Mode::Compose, false),
            KeyIntent::Compose(BufOp::Insert('a'))
        );
    }

    #[test]
    fn ctrl_v_reads_the_clipboard_while_an_editor_is_open() {
        let c = KeyModifiers::CONTROL;
        for mode in [Mode::Compose, Mode::Edit] {
            assert_eq!(
                classify_key(KeyCode::Char('v'), c, mode, false),
                KeyIntent::PasteClipboard
            );
        }
        // Browsing the queue has nothing to paste into.
        assert_eq!(
            classify_key(KeyCode::Char('v'), c, Mode::Browse, false),
            KeyIntent::Ignore
        );
    }

    #[test]
    fn escape_cancels_the_turn_only_from_the_composer() {
        let n = KeyModifiers::NONE;
        // In the composer Esc is the turn's cancel — distinct from Ctrl-C, so
        // the loops can cancel without clearing the draft or arming the exit.
        assert_eq!(
            classify_key(KeyCode::Esc, n, Mode::Compose, false),
            KeyIntent::CancelTurn
        );
        assert_eq!(
            classify_key(KeyCode::Esc, n, Mode::Compose, true),
            KeyIntent::CancelTurn
        );
        // Inline editing keeps Esc as "discard this edit"; browsing ignores it.
        assert_eq!(
            classify_key(KeyCode::Esc, n, Mode::Edit, false),
            KeyIntent::EditCancel
        );
        assert_eq!(
            classify_key(KeyCode::Esc, n, Mode::Browse, false),
            KeyIntent::Ignore
        );
    }

    #[test]
    fn ctrl_enter_and_ctrl_q_queue_a_follow_up() {
        let c = KeyModifiers::CONTROL;
        // Ctrl+Enter (kitty protocol only) and Ctrl+Q (everywhere else) are
        // the same intent, so the chord that works is never the mystery.
        assert_eq!(
            classify_key(KeyCode::Enter, c, Mode::Compose, false),
            KeyIntent::QueueFollowUp
        );
        assert_eq!(
            classify_key(KeyCode::Char('q'), c, Mode::Compose, false),
            KeyIntent::QueueFollowUp
        );
        // Plain Enter still submits (mid-turn: steers) — unchanged.
        assert_eq!(
            classify_key(KeyCode::Enter, KeyModifiers::NONE, Mode::Compose, false),
            KeyIntent::ComposerSubmit
        );
        // Neither chord hijacks the inline item editor or the queue browser.
        assert_eq!(
            classify_key(KeyCode::Char('q'), c, Mode::Edit, false),
            KeyIntent::Ignore
        );
        assert_eq!(
            classify_key(KeyCode::Enter, c, Mode::Browse, false),
            KeyIntent::Ignore
        );
    }

    #[test]
    fn alt_up_pulls_the_newest_queued_item_back() {
        assert_eq!(
            classify_key(KeyCode::Up, KeyModifiers::ALT, Mode::Compose, true),
            KeyIntent::PullQueued
        );
        // Unmodified Up is still line-move / history / queue focus.
        assert_eq!(
            classify_key(KeyCode::Up, KeyModifiers::NONE, Mode::Compose, true),
            KeyIntent::ComposeUp
        );
    }

    #[test]
    fn tab_is_a_compose_completion() {
        assert_eq!(
            classify_key(KeyCode::Tab, KeyModifiers::NONE, Mode::Compose, false),
            KeyIntent::Complete
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
        // On a non-empty composer Ctrl-D is readline's forward delete.
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Compose, false),
            KeyIntent::Compose(BufOp::Delete)
        );
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Browse, true),
            KeyIntent::Ignore
        );
    }

    #[test]
    fn word_chords_map_in_compose_and_edit() {
        let a = KeyModifiers::ALT;
        let c = KeyModifiers::CONTROL;
        for mode in [Mode::Compose, Mode::Edit] {
            let intent = |op| match mode {
                Mode::Compose => KeyIntent::Compose(op),
                _ => KeyIntent::Edit(op),
            };
            // Option/Alt+arrows and the ESC-b / ESC-f forms hop words.
            for (code, mods) in [
                (KeyCode::Left, a),
                (KeyCode::Char('b'), a),
                (KeyCode::Left, c),
            ] {
                assert_eq!(
                    classify_key(code, mods, mode, false),
                    intent(BufOp::WordLeft)
                );
            }
            for (code, mods) in [
                (KeyCode::Right, a),
                (KeyCode::Char('f'), a),
                (KeyCode::Right, c),
            ] {
                assert_eq!(
                    classify_key(code, mods, mode, false),
                    intent(BufOp::WordRight)
                );
            }
            for (code, mods) in [
                (KeyCode::Backspace, a),
                (KeyCode::Backspace, c),
                (KeyCode::Char('w'), c),
            ] {
                assert_eq!(
                    classify_key(code, mods, mode, false),
                    intent(BufOp::DeleteWordBack)
                );
            }
            assert_eq!(
                classify_key(KeyCode::Char('d'), a, mode, false),
                intent(BufOp::DeleteWordForward)
            );
        }
    }

    #[test]
    fn readline_ctrl_chords_map_to_line_edits() {
        let c = KeyModifiers::CONTROL;
        for (ch, op) in [
            ('a', BufOp::Home),
            ('e', BufOp::End),
            ('b', BufOp::Left),
            ('f', BufOp::Right),
            ('k', BufOp::KillToEnd),
            ('u', BufOp::KillToStart),
        ] {
            assert_eq!(
                classify_key(KeyCode::Char(ch), c, Mode::Compose, false),
                KeyIntent::Compose(op)
            );
            assert_eq!(
                classify_key(KeyCode::Char(ch), c, Mode::Edit, false),
                KeyIntent::Edit(op)
            );
        }
    }

    #[test]
    fn alt_and_ctrl_chords_are_ignored_while_browsing() {
        for mods in [KeyModifiers::ALT, KeyModifiers::CONTROL] {
            assert_eq!(
                classify_key(KeyCode::Backspace, mods, Mode::Browse, false),
                KeyIntent::Ignore
            );
        }
    }
}
