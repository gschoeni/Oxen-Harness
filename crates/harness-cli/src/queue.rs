//! A small, editable queue of pending prompts for the REPL.
//!
//! A terminal turn streams to the screen and blocks the prompt while it runs,
//! so rather than typing ahead "live" (which would fight the spinner for the
//! terminal), the user *stacks* messages with `/queue add`, edits/reorders them
//! before they run, and then sends the whole batch with `/queue run`. Indices
//! shown to the user are 1-based.

/// An ordered list of prompts waiting to be sent to the agent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MessageQueue {
    items: Vec<String>,
    /// Every prompt added this session (never popped by drains). The REPL folds
    /// these into its readline history so queued prompts — not just ones typed at
    /// the idle prompt — are recallable with Up-arrow, including across sessions.
    authored: Vec<String>,
}

impl MessageQueue {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Append a message; returns its 1-based position.
    pub fn add(&mut self, msg: impl Into<String>) -> usize {
        let msg = msg.into();
        self.authored.push(msg.clone());
        self.items.push(msg);
        self.items.len()
    }

    /// Take the prompts authored since the last call, for the REPL to fold into
    /// its readline history.
    pub fn take_authored(&mut self) -> Vec<String> {
        std::mem::take(&mut self.authored)
    }

    /// Replace the message at 1-based `pos`.
    pub fn edit(&mut self, pos: usize, msg: impl Into<String>) -> Result<(), String> {
        let idx = self.index(pos)?;
        self.items[idx] = msg.into();
        Ok(())
    }

    /// Remove and return the message at 1-based `pos`.
    pub fn remove(&mut self, pos: usize) -> Result<String, String> {
        let idx = self.index(pos)?;
        Ok(self.items.remove(idx))
    }

    /// Remove and return the next message to execute.
    pub fn pop_front(&mut self) -> Option<String> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.items.remove(0))
        }
    }

    /// Swap the message at 1-based `pos` with its neighbour in `dir`
    /// (`-1` = up, `+1` = down). Out-of-range moves are a no-op error.
    pub fn move_by(&mut self, pos: usize, dir: i64) -> Result<(), String> {
        let idx = self.index(pos)? as i64;
        let target = idx + dir;
        if target < 0 || target as usize >= self.items.len() {
            return Err("already at the edge of the queue".to_string());
        }
        self.items.swap(idx as usize, target as usize);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Drain every message at once, leaving the queue empty. (The REPL now
    /// drains one message at a time so the user can keep stacking mid-turn, but
    /// this batch form stays part of the queue's API.)
    #[allow(dead_code)]
    pub fn take_all(&mut self) -> Vec<String> {
        std::mem::take(&mut self.items)
    }

    fn index(&self, pos: usize) -> Result<usize, String> {
        if pos == 0 || pos > self.items.len() {
            return Err(format!(
                "no message #{pos} in the queue (have 1..={})",
                self.items.len()
            ));
        }
        Ok(pos - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_appends_and_returns_position() {
        let mut q = MessageQueue::default();
        assert_eq!(q.add("one"), 1);
        assert_eq!(q.add("two"), 2);
        assert_eq!(q.items(), &["one".to_string(), "two".to_string()]);
        assert!(!q.is_empty());
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn edit_replaces_in_place_and_validates_bounds() {
        let mut q = MessageQueue::default();
        q.add("one");
        q.add("two");
        assert!(q.edit(1, "uno").is_ok());
        assert_eq!(q.items()[0], "uno");
        assert!(q.edit(0, "x").is_err());
        assert!(q.edit(3, "x").is_err());
    }

    #[test]
    fn remove_pulls_message_and_shifts_rest() {
        let mut q = MessageQueue::default();
        q.add("one");
        q.add("two");
        q.add("three");
        assert_eq!(q.remove(2).unwrap(), "two");
        assert_eq!(q.items(), &["one".to_string(), "three".to_string()]);
        assert!(q.remove(9).is_err());
    }

    #[test]
    fn move_by_swaps_neighbours_and_guards_edges() {
        let mut q = MessageQueue::default();
        q.add("one");
        q.add("two");
        assert!(q.move_by(2, -1).is_ok());
        assert_eq!(q.items(), &["two".to_string(), "one".to_string()]);
        assert!(q.move_by(1, -1).is_err()); // already at top
        assert!(q.move_by(2, 1).is_err()); // already at bottom
    }

    #[test]
    fn stacking_then_front_draining_preserves_order() {
        // Mirrors the live composer: each submitted line is `add`ed (stacked),
        // and the turn loop drains the front with `pop_front` — even when more
        // are stacked between drains, earlier messages still send first.
        let mut q = MessageQueue::default();
        q.add("first");
        q.add("second");
        assert_eq!(q.pop_front(), Some("first".to_string()));
        q.add("third"); // stacked mid-drain
        assert_eq!(q.pop_front(), Some("second".to_string()));
        assert_eq!(q.pop_front(), Some("third".to_string()));
        assert_eq!(q.pop_front(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn authored_log_captures_adds_and_survives_draining() {
        let mut q = MessageQueue::default();
        q.add("one");
        q.add("two");
        // Draining items for execution must not lose the authored history — the
        // REPL still needs to fold both prompts into readline history.
        let _ = q.pop_front();
        assert_eq!(
            q.take_authored(),
            vec!["one".to_string(), "two".to_string()]
        );
        // Taking is one-shot: nothing left to fold in next time.
        assert!(q.take_authored().is_empty());
    }

    #[test]
    fn take_all_drains_to_empty() {
        let mut q = MessageQueue::default();
        q.add("one");
        q.add("two");
        assert_eq!(q.take_all(), vec!["one".to_string(), "two".to_string()]);
        assert!(q.is_empty());
    }
}
