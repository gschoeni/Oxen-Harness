//! Shared [`KeyAction`] dispatch for the two composer loops.
//!
//! The idle prompt and the mid-turn loop handle most key actions identically
//! (queue edits, deletes, redraw marking) — only Submit/Interrupt/Exit differ.
//! [`apply_action`] applies the identical part once (including the easy-to-get
//! wrong `idx + 1` mapping from focus index to 1-based queue position) and
//! returns the loop-specific [`Residual`] for the caller to decide. Handlers
//! only mark the screen dirty; the loop flushes one repaint per event.

use crate::queue::MessageQueue;

use super::keys::KeyAction;
use super::Live;

/// The loop-specific leftovers of a key action.
pub(super) enum Residual {
    /// The composer submitted this text (idle: run it; mid-turn: steer/queue).
    Submit(String),
    /// Ctrl-C (idle: staged clear/arm/exit; mid-turn: cancel the turn).
    Interrupt,
    /// Ctrl-D on an empty composer — end the session.
    Exit,
}

/// Apply the loop-independent effects of `action` against the live state and
/// the authoritative queue, marking the screen dirty as needed. Returns the
/// residual the caller must handle, or `None` when fully handled here.
pub(super) fn apply_action(
    live: &mut Live,
    queue: &mut MessageQueue,
    action: KeyAction,
) -> Option<Residual> {
    match action {
        KeyAction::None => None,
        KeyAction::Redraw => {
            live.request_paint();
            None
        }
        KeyAction::BeginEdit => {
            if let Some(i) = live.focused_item() {
                if let Some(text) = queue.items().get(i) {
                    live.begin_edit(text);
                }
            }
            live.request_paint();
            None
        }
        KeyAction::SaveEdit => {
            if let Some((idx, text)) = live.take_edit() {
                // Queue positions are 1-based; focus indexes are 0-based.
                let _ = queue.edit(idx + 1, text);
                live.sync_queue(queue.items());
            }
            live.request_paint();
            None
        }
        KeyAction::CancelEdit => {
            live.cancel_edit();
            live.request_paint();
            None
        }
        KeyAction::DeleteFocused => {
            if let Some(i) = live.focused_item() {
                let _ = queue.remove(i + 1);
            }
            live.sync_queue(queue.items());
            live.request_paint();
            None
        }
        KeyAction::Submit(line) => Some(Residual::Submit(line)),
        KeyAction::Interrupt => Some(Residual::Interrupt),
        KeyAction::Exit => Some(Residual::Exit),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::super::layout::Focus;
    use super::super::test_support::{key, live};
    use super::*;

    fn queue_of(items: &[&str]) -> MessageQueue {
        let mut q = MessageQueue::default();
        for i in items {
            q.add(i.to_string());
        }
        q
    }

    #[test]
    fn save_edit_writes_the_one_based_queue_slot() {
        let mut l = live(80, 24);
        let mut q = queue_of(&["first", "second"]);
        l.sync_queue(q.items());
        l.focus = Focus::Item(1);
        l.begin_edit("second");
        for ch in " thoughts".chars() {
            l.handle_key(key(KeyCode::Char(ch)), q.len());
        }
        let action = l.handle_key(key(KeyCode::Enter), q.len());
        assert!(apply_action(&mut l, &mut q, action).is_none());
        assert_eq!(q.items(), &["first".to_string(), "second thoughts".into()]);
        assert_eq!(l.previews[1], "second thoughts");
    }

    #[test]
    fn delete_removes_the_focused_item_and_reclamps() {
        let mut l = live(80, 24);
        let mut q = queue_of(&["a", "b"]);
        l.sync_queue(q.items());
        l.focus = Focus::Item(1);
        let action = l.handle_key(key(KeyCode::Char('d')), q.len());
        assert!(apply_action(&mut l, &mut q, action).is_none());
        assert_eq!(q.items(), &["a".to_string()]);
        assert_eq!(l.focus, Focus::Item(0));
    }

    #[test]
    fn submit_interrupt_and_exit_are_left_to_the_loop() {
        let mut l = live(80, 24);
        let mut q = MessageQueue::default();
        assert!(matches!(
            apply_action(&mut l, &mut q, KeyAction::Submit("hi".into())),
            Some(Residual::Submit(s)) if s == "hi"
        ));
        assert!(matches!(
            apply_action(&mut l, &mut q, KeyAction::Interrupt),
            Some(Residual::Interrupt)
        ));
        assert!(matches!(
            apply_action(&mut l, &mut q, KeyAction::Exit),
            Some(Residual::Exit)
        ));
    }
}
