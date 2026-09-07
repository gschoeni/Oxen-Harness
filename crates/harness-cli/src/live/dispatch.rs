//! Shared [`KeyAction`] dispatch for the two composer loops.
//!
//! The idle prompt and the mid-turn loop handle most key actions identically
//! (queue edits, deletes, redraw marking) — only Submit/Interrupt/Exit differ.
//! [`apply_action`] applies the identical part once (including the easy-to-get
//! wrong `idx + 1` mapping from focus index to 1-based queue position) and
//! returns the loop-specific [`Residual`] for the caller to decide. Handlers
//! only mark the screen dirty; the loop flushes one repaint per event.

use crate::queue::MessageQueue;
use crate::render::truncate;

use super::composer::expand_pastes;
use super::keys::KeyAction;
use super::turn::stackable;
use super::Live;

/// The loop-specific leftovers of a key action.
pub(super) enum Residual {
    /// The composer submitted this text (idle: run it; mid-turn: steer/queue).
    Submit(String),
    /// Ctrl-C (idle: staged clear/arm/exit; mid-turn: cancel the turn).
    Interrupt,
    /// Esc — cancel a running turn (idle: nothing). Never touches the draft or
    /// the staged exit, so it is safe to lean on.
    CancelTurn,
    /// Ctrl-D on an empty composer — end the session.
    Exit,
    /// Ctrl+G — edit the draft in `$EDITOR` (the idle loop hands the terminal
    /// over; mid-turn it is ignored).
    ExternalEditor(String),
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
        // Queueing behaves identically idle and mid-turn — the text runs after
        // whatever is (or isn't) in flight — so it is fully handled here.
        KeyAction::QueueFollowUp(line) => {
            queue_follow_up(live, queue, &line);
            live.request_paint();
            None
        }
        KeyAction::ExpandLast => {
            live.expand_last_result();
            None
        }
        KeyAction::ExternalEditor => Some(Residual::ExternalEditor(live.composer_draft())),
        KeyAction::PullQueued => {
            // The newest item comes back out for another pass; queue positions
            // are 1-based, so the last one is at `len`.
            if let Ok(text) = queue.remove(queue.len()) {
                live.composer.set_text(&text);
                live.sync_queue(queue.items());
                live.refresh_completion();
            }
            live.request_paint();
            None
        }
        KeyAction::Submit(line) => Some(Residual::Submit(line)),
        KeyAction::Interrupt => Some(Residual::Interrupt),
        KeyAction::CancelTurn => Some(Residual::CancelTurn),
        KeyAction::Exit => Some(Residual::Exit),
    }
}

/// Stack `line` as a follow-up prompt and echo it, mirroring how the mid-turn
/// steering echo reads. A recognized `/command` can't be stacked (the queue
/// drains as chat to the model), so it stays in the composer with a note —
/// the same rule steering uses.
fn queue_follow_up(live: &mut Live, queue: &mut MessageQueue, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let ui = live.ui.clone();
    if !stackable(trimmed) {
        live.print_line(&format!(
            "  {} {}",
            ui.brown("⛺"),
            ui.dim("commands don't stack in the wagon — kept in the composer to run on its own"),
        ));
        live.composer.set_text(trimmed);
        return;
    }
    // Chips stand in for bulky pastes on screen; the queue carries the real
    // text, since it goes straight to the model when it drains.
    queue.add(expand_pastes(trimmed));
    live.sync_queue(queue.items());
    live.print_line(&format!(
        "  {} {}",
        ui.brown("⛺ queued:"),
        ui.cream(&truncate(&first_line(trimmed), 80)),
    ));
}

/// A one-line preview of a possibly multi-line submission, so an echo can
/// never smear across the scroll region.
fn first_line(text: &str) -> String {
    match text.split_once('\n') {
        Some((head, _)) => format!("{head} …"),
        None => text.to_string(),
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
    fn a_follow_up_is_queued_and_echoed_not_steered() {
        let mut l = live(80, 24);
        let mut q = MessageQueue::default();
        let action = KeyAction::QueueFollowUp("  then update the docs  ".into());
        assert!(apply_action(&mut l, &mut q, action).is_none());
        // It lands on the queue (trimmed) and shows up in the pinned list.
        assert_eq!(q.items(), &["then update the docs".to_string()]);
        assert_eq!(l.previews, vec!["then update the docs".to_string()]);

        // An empty line queues nothing.
        assert!(apply_action(&mut l, &mut q, KeyAction::QueueFollowUp("   ".into())).is_none());
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn a_command_cannot_be_queued_and_stays_in_the_composer() {
        let mut l = live(80, 24);
        let mut q = MessageQueue::default();
        let action = KeyAction::QueueFollowUp("/model claude-sonnet-4-6".into());
        assert!(apply_action(&mut l, &mut q, action).is_none());
        // The queue drains as chat to the model, so a command can't ride it —
        // it waits in the composer instead of being silently mangled.
        assert!(q.is_empty());
        assert_eq!(l.composer.text(), "/model claude-sonnet-4-6");
    }

    #[test]
    fn alt_up_moves_the_newest_queued_item_back_into_the_composer() {
        let mut l = live(80, 24);
        let mut q = queue_of(&["first", "second"]);
        l.sync_queue(q.items());
        assert!(apply_action(&mut l, &mut q, KeyAction::PullQueued).is_none());
        assert_eq!(l.composer.text(), "second");
        assert_eq!(q.items(), &["first".to_string()]);
        assert_eq!(l.previews, vec!["first".to_string()]);

        // Pulling from an empty queue is a no-op, not a panic.
        let mut empty = MessageQueue::default();
        let mut l = live(80, 24);
        assert!(apply_action(&mut l, &mut empty, KeyAction::PullQueued).is_none());
        assert!(l.composer.is_empty());
    }

    #[test]
    fn a_chipped_paste_reaches_the_queue_as_its_real_text() {
        let mut l = live(80, 24);
        let mut q = MessageQueue::default();
        let dump = "line\n".repeat(30);
        let chip = super::super::composer::stage_paste(&dump);
        let action = KeyAction::QueueFollowUp(format!("explain {chip}"));
        assert!(apply_action(&mut l, &mut q, action).is_none());
        // The chip is composer shorthand; the model gets the paste itself.
        assert_eq!(q.items()[0], format!("explain {dump}"));
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
        // Esc is its own residual — the loops must be able to tell it from
        // Ctrl-C, which also clears the draft and arms the exit.
        assert!(matches!(
            apply_action(&mut l, &mut q, KeyAction::CancelTurn),
            Some(Residual::CancelTurn)
        ));
    }
}
