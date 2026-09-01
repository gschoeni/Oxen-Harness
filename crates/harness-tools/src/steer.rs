//! The "the user just said something" signal.
//!
//! A foreground `run_shell` call blocks the whole turn: while it waits, the
//! agent cannot read a new message, let alone act on it. That is fine for a
//! command that returns in a second and maddening for one that doesn't — the
//! user types "stop, wrong directory" and watches it scroll past.
//!
//! The steer signal lets a waiting tool notice that. The host holds a
//! [`SteerNotifier`] and bumps it when input arrives; tools hold a
//! [`SteerSignal`] and race their wait against it, cutting the wait short
//! (`run_shell` backgrounds the command rather than killing it, so nothing is
//! lost). It is deliberately a bare counter, not a message channel: the tool
//! only needs to know that *something* arrived, and the host still delivers
//! the message itself through its normal path.

use std::sync::Arc;

use tokio::sync::watch;

/// The host end: bump this when the user sends something mid-turn.
///
/// Cloneable so every host surface (a REPL reader, an HTTP handler) can hold
/// one; they all feed the same counter.
#[derive(Clone)]
pub struct SteerNotifier(Arc<watch::Sender<u64>>);

/// The tool end: a handle that resolves once the counter moves.
#[derive(Clone)]
pub struct SteerSignal(watch::Receiver<u64>);

/// Create a linked notifier/signal pair.
pub fn steer_channel() -> (SteerNotifier, SteerSignal) {
    let (tx, rx) = watch::channel(0);
    (SteerNotifier(Arc::new(tx)), SteerSignal(rx))
}

impl SteerNotifier {
    /// Record that the user steered. Never blocks, and never fails — with no
    /// tool currently waiting the bump is simply the value the next
    /// [`SteerSignal::wait`] starts from.
    pub fn notify(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }
}

impl SteerSignal {
    /// Resolve as soon as the counter differs from the value it holds now.
    ///
    /// A steer that arrived *before* this call does not count: the baseline is
    /// read at the start of the wait, so a tool never trips over the previous
    /// turn's interruption. If the notifier is gone the future simply never
    /// resolves, leaving whatever it was raced against to decide.
    pub async fn wait(&mut self) {
        let start = *self.0.borrow_and_update();
        while self.0.changed().await.is_ok() {
            if *self.0.borrow_and_update() != start {
                return;
            }
        }
        std::future::pending::<()>().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn wait_resolves_when_the_host_notifies() {
        let (notifier, mut signal) = steer_channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            notifier.notify();
        });
        tokio::time::timeout(Duration::from_secs(5), signal.wait())
            .await
            .expect("steer should fire");
    }

    #[tokio::test]
    async fn an_earlier_steer_does_not_satisfy_a_later_wait() {
        let (notifier, mut signal) = steer_channel();
        notifier.notify();
        // The bump happened before the wait started, so this must not resolve.
        let early = tokio::time::timeout(Duration::from_millis(50), signal.wait()).await;
        assert!(early.is_err(), "a stale steer should not fire a new wait");
        // …but one that arrives during the wait does.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            notifier.notify();
        });
        tokio::time::timeout(Duration::from_secs(5), signal.wait())
            .await
            .expect("a fresh steer should fire");
    }

    #[tokio::test]
    async fn wait_never_resolves_once_the_notifier_is_dropped() {
        let (notifier, mut signal) = steer_channel();
        drop(notifier);
        let out = tokio::time::timeout(Duration::from_millis(50), signal.wait()).await;
        assert!(out.is_err(), "a dead notifier must not look like a steer");
    }
}
