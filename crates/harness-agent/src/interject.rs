//! Mid-turn steering: messages the user sends while a turn is running.
//!
//! A host pushes into an [`Interjections`] handle (cloned off the agent
//! before the turn starts, so no lock on the agent is needed); the turn loop
//! drains it at safe points — the top of every model/tool round, and just
//! before the turn would end — so the model sees the message *during* the
//! work rather than after it. Each drained message becomes its own framed
//! user message in the transcript (FIFO, never merged), and a drain at the
//! end of a turn forces one more model round so a message that arrived while
//! the final reply streamed is never silently dropped.
//!
//! Anything still in the buffer when a turn ends (it was cancelled, or the
//! message landed in the instant after the final drain) is the host's to
//! recover — see [`Interjections::take_all`] — typically by queueing it as
//! the next prompt.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A cloneable handle to the agent's mid-turn message buffer.
#[derive(Clone, Default)]
pub struct Interjections {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl Interjections {
    /// Queue a message for delivery at the turn's next safe point. Callable
    /// from any thread while the turn runs.
    pub fn push(&self, text: impl Into<String>) {
        self.inner
            .lock()
            .expect("interjection lock")
            .push_back(text.into());
    }

    /// Drain everything queued, in arrival order.
    pub fn take_all(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("interjection lock")
            .drain(..)
            .collect()
    }

    /// How many messages are waiting.
    pub fn pending(&self) -> usize {
        self.inner.lock().expect("interjection lock").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_take_preserves_order_and_clears() {
        let ij = Interjections::default();
        ij.push("first");
        ij.push("second");
        assert_eq!(ij.pending(), 2);
        assert_eq!(ij.take_all(), vec!["first", "second"]);
        assert_eq!(ij.pending(), 0);
        assert!(ij.take_all().is_empty());
    }

    #[test]
    fn clones_share_the_buffer() {
        let ij = Interjections::default();
        let handle = ij.clone();
        handle.push("from the host");
        assert_eq!(ij.take_all(), vec!["from the host"]);
    }
}
