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
    Home,
    End,
}

/// The semantic intent of a keystroke, decided purely from the key + current
/// [`Mode`]. Keeping this separate from the IO lets the whole key map be
/// unit-tested without a terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum KeyIntent {
    Ignore,
    Interrupt,
    Exit,
    Compose(BufOp),
    /// Insert a hard line break in the composer (Alt/Shift+Enter, Ctrl+J).
    ComposeNewline,
    ComposerSubmit,
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
/// interrupts; Ctrl-D exits only on an empty composer.
pub(super) fn classify_key(
    code: KeyCode,
    mods: KeyModifiers,
    mode: Mode,
    composer_empty: bool,
) -> KeyIntent {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    if ctrl {
        return match code {
            KeyCode::Char('c') => KeyIntent::Interrupt,
            KeyCode::Char('d') if mode == Mode::Compose && composer_empty => KeyIntent::Exit,
            // Ctrl-J is a portable "insert newline" that survives terminals which
            // don't distinguish Shift/Alt+Enter.
            KeyCode::Char('j') if mode == Mode::Compose => KeyIntent::ComposeNewline,
            _ => KeyIntent::Ignore,
        };
    }
    match mode {
        Mode::Compose => match code {
            // Alt/Shift+Enter add a line; plain Enter sends (or queues).
            KeyCode::Enter if alt || mods.contains(KeyModifiers::SHIFT) => {
                KeyIntent::ComposeNewline
            }
            KeyCode::Enter => KeyIntent::ComposerSubmit,
            KeyCode::Tab => KeyIntent::Complete,
            KeyCode::Up => KeyIntent::ComposeUp,
            KeyCode::Down => KeyIntent::ComposeDown,
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
pub(super) fn apply_buf(c: &mut Composer, op: BufOp) {
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
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Compose, false),
            KeyIntent::Ignore
        );
        assert_eq!(
            classify_key(KeyCode::Char('d'), c, Mode::Browse, true),
            KeyIntent::Ignore
        );
    }
}
