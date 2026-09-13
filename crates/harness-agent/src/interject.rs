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
    /// Woken on every push, so a tool waiting on a slow command (the shell)
    /// can background it and let the message through instead of making the
    /// user wait out the command.
    notifier: Option<harness_tools::SteerNotifier>,
}

impl Interjections {
    /// A buffer that also pokes `notifier` on every push — the registry's
    /// steer channel, so `run_shell` backgrounds early when the user speaks.
    pub fn with_notifier(notifier: Option<harness_tools::SteerNotifier>) -> Self {
        Self {
            inner: Arc::default(),
            notifier,
        }
    }

    /// Queue a message for delivery at the turn's next safe point. Callable
    /// from any thread while the turn runs.
    ///
    /// The steer is *held*, not just bumped: a tool that starts waiting
    /// later in the same round (the model was still streaming its `run_shell`
    /// call when the user spoke) still backgrounds at once, instead of
    /// blocking the full timeout on a baseline taken after the message
    /// arrived. [`take_all`] releases it.
    ///
    /// [`take_all`]: Self::take_all
    pub fn push(&self, text: impl Into<String>) {
        self.inner
            .lock()
            .expect("interjection lock")
            .push_back(text.into());
        if let Some(notifier) = &self.notifier {
            notifier.hold();
        }
    }

    /// Drain everything queued, in arrival order. Releases the held steer:
    /// the messages are on their way to the model, so a tool's next wait
    /// runs its course unless the user speaks again.
    pub fn take_all(&self) -> Vec<String> {
        let drained: Vec<String> = self
            .inner
            .lock()
            .expect("interjection lock")
            .drain(..)
            .collect();
        if let Some(notifier) = &self.notifier {
            notifier.release();
        }
        drained
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

    /// The bug this guards against: the user speaks while the model is still
    /// streaming its `run_shell` call, so no tool is waiting yet. The wait
    /// that starts afterwards must still be cut short — and, once the
    /// message has been drained for delivery, a later wait must not be.
    #[tokio::test]
    async fn a_message_that_arrives_before_the_tool_waits_still_cuts_the_wait_short() {
        use std::time::Duration;
        let (notifier, mut signal) = harness_tools::steer_channel();
        let ij = Interjections::with_notifier(Some(notifier));
        ij.push("stop, wrong directory");
        tokio::time::timeout(Duration::from_millis(200), signal.wait())
            .await
            .expect("a pending interjection must cut a later wait short");
        assert_eq!(ij.take_all(), vec!["stop, wrong directory"]);
        let after = tokio::time::timeout(Duration::from_millis(50), signal.wait()).await;
        assert!(after.is_err(), "drained: the wait runs its course again");
    }

    #[test]
    fn clones_share_the_buffer() {
        let ij = Interjections::default();
        let handle = ij.clone();
        handle.push("from the host");
        assert_eq!(ij.take_all(), vec!["from the host"]);
    }
}
