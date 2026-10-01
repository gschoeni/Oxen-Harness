//! The transport-agnostic host layer between the agent core and a UI.
//!
//! `harness_agent::Agent` is a single-session loop; every front end also
//! needs the same orchestration around it: a multi-session agent cache with
//! lazy rehydration, turn driving with cancel tokens, the ask/approval
//! round-trips, model/client selection, and the translation of in-process
//! events onto the wire. That layer used to live inside the Tauri app; this
//! crate owns it, generic over one seam:
//!
//! - [`EventSink`] — where protocol events go. The desktop implements it with
//!   `AppHandle::emit`, the HTTP server with an SSE broadcast, a test with a
//!   `Vec`.
//!
//! Everything a client sends back (question answers, approval decisions)
//! arrives through [`SessionService`] methods and meets its waiting turn via
//! the [`PendingMap`]s — the same id-keyed oneshot pattern on every
//! transport.

mod agents;
mod bridges;
mod service;
mod study;
mod threads;
pub mod translate;
mod view_development;
pub mod workbench;

use harness_protocol::ProtocolEvent;

pub use bridges::{
    HostApprover, HostAsker, HostCanvasSink, HostFleetSink, HostViewerSink, NoScreenshotLens,
    NullAsker, NullCanvasSink, NullFleetSink, NullPreviewLens, NullPreviewSink, NullViewerSink,
    ProtocolPreviewSink,
};
pub use service::{
    launch_dir, ClientFactory, HostHooks, SessionNotify, SessionService, SessionServiceBuilder,
    SurfaceFactory,
};

/// Where protocol events go — the one seam a host transport implements.
/// Implementations must be cheap and non-blocking: events are emitted inline
/// from the streaming turn loop.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: ProtocolEvent);
}

/// Outstanding host round-trips (questions, approvals) awaiting a client
/// answer, keyed by id: registering parks a oneshot receiver, the client's
/// answer finds it by id. A forgotten/dropped entry reads as "no interactive
/// user" on the waiting side.
///
/// Each entry also remembers the event the client was told to answer, so a
/// client that arrives *after* it was emitted (a reloaded desktop webview, a
/// reconnected HTTP client) can be handed the same request again instead of
/// leaving the turn parked on a question nobody can see.
pub struct PendingMap<T> {
    inner: std::sync::Mutex<PendingEntries<T>>,
    counter: std::sync::atomic::AtomicU64,
}

/// One parked round-trip: who is waiting, and what the client was asked.
struct Parked<T> {
    tx: tokio::sync::oneshot::Sender<T>,
    session: String,
    /// Registration order, so a replay lists a session's requests as asked.
    ordinal: u64,
    /// The request as emitted to the client; `None` between `register` and
    /// `announce` (a bridge that builds the event after parking).
    request: Option<ProtocolEvent>,
}

type PendingEntries<T> = std::collections::HashMap<String, Parked<T>>;

impl<T> Default for PendingMap<T> {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
            counter: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl<T> PendingMap<T> {
    /// Park a new round-trip for `session`, returning its id and the receiver
    /// to await.
    pub fn register(
        &self,
        prefix: &str,
        session: &str,
    ) -> (String, tokio::sync::oneshot::Receiver<T>) {
        let ordinal = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("{prefix}{ordinal}");
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner.lock().expect("pending map poisoned").insert(
            id.clone(),
            Parked {
                tx,
                session: session.to_string(),
                ordinal,
                request: None,
            },
        );
        (id, rx)
    }

    /// Record the request event emitted for a parked round-trip, so
    /// [`Self::pending_for`] can replay it to a client that missed it.
    pub fn announce(&self, id: &str, request: ProtocolEvent) {
        if let Some(entry) = self.inner.lock().expect("pending map poisoned").get_mut(id) {
            entry.request = Some(request);
        }
    }

    /// The requests `session`'s turn is still parked on, oldest first.
    pub fn pending_for(&self, session: &str) -> Vec<ProtocolEvent> {
        let inner = self.inner.lock().expect("pending map poisoned");
        let mut parked: Vec<&Parked<T>> = inner
            .values()
            .filter(|entry| entry.session == session)
            .collect();
        parked.sort_by_key(|entry| entry.ordinal);
        parked
            .into_iter()
            .filter_map(|entry| entry.request.clone())
            .collect()
    }

    /// Deliver the client's answer to a parked round-trip. Returns false when
    /// the id is unknown (already answered, cancelled, or evicted) — ignored
    /// by design.
    pub fn deliver(&self, id: &str, value: T) -> bool {
        let parked = self.inner.lock().expect("pending map poisoned").remove(id);
        match parked {
            Some(entry) => entry.tx.send(value).is_ok(),
            None => false,
        }
    }

    /// Drop a parked round-trip without answering (chat evicted, client gone):
    /// the waiting side sees a closed channel and treats it as "no answer".
    pub fn forget(&self, id: &str) {
        self.inner.lock().expect("pending map poisoned").remove(id);
    }

    /// Drop every round-trip `session`'s turn is parked on (the turn was
    /// stopped or ended): each waiting tool sees a closed channel and returns
    /// with "no answer", so a stop can unwind a turn that is blocked on the
    /// user. Returns how many were dropped.
    pub fn forget_session(&self, session: &str) -> usize {
        let mut inner = self.inner.lock().expect("pending map poisoned");
        let before = inner.len();
        inner.retain(|_, entry| entry.session != session);
        before - inner.len()
    }
}

/// Questions awaiting a client answer (`ask_user_question`).
pub type PendingQuestions = std::sync::Arc<PendingMap<Vec<harness_tools::QuestionAnswer>>>;
/// Permission approvals awaiting a client decision.
pub type PendingApprovals = std::sync::Arc<PendingMap<harness_protocol::ApprovalAnswer>>;

#[cfg(test)]
mod pending_tests {
    use super::PendingMap;
    use harness_protocol::ProtocolEvent;

    fn question(session: &str, id: &str) -> ProtocolEvent {
        ProtocolEvent::Question {
            session: session.into(),
            id: id.into(),
            questions: vec![],
        }
    }

    #[test]
    fn pending_for_replays_announced_requests_in_order_per_session() {
        let map: PendingMap<u8> = PendingMap::default();
        let (a, _rx_a) = map.register("q", "s1");
        let (b, _rx_b) = map.register("q", "s2");
        let (c, _rx_c) = map.register("q", "s1");
        map.announce(&a, question("s1", &a));
        map.announce(&b, question("s2", &b));
        map.announce(&c, question("s1", &c));

        let ids: Vec<String> = map
            .pending_for("s1")
            .into_iter()
            .map(|e| match e {
                ProtocolEvent::Question { id, .. } => id,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(ids, vec![a.clone(), c.clone()]);
        assert_eq!(map.pending_for("s2").len(), 1);
        assert!(map.pending_for("nobody").is_empty());

        // Answered → no longer pending; a registered-but-unannounced entry
        // has nothing to replay.
        assert!(map.deliver(&a, 1));
        let (d, _rx_d) = map.register("q", "s1");
        let _ = d;
        assert_eq!(map.pending_for("s1").len(), 1);
    }

    #[tokio::test]
    async fn forget_session_unparks_every_waiter_of_that_session_only() {
        let map: PendingMap<u8> = PendingMap::default();
        let (_a, rx_a) = map.register("q", "s1");
        let (_b, rx_b) = map.register("q", "s1");
        let (c, rx_c) = map.register("q", "s2");

        assert_eq!(map.forget_session("s1"), 2);
        assert!(
            rx_a.await.is_err(),
            "a stopped turn's waiter sees a closed channel"
        );
        assert!(rx_b.await.is_err());
        assert!(map.pending_for("s1").is_empty());

        // The other session's round-trip still answers normally.
        assert!(map.deliver(&c, 7));
        assert_eq!(rx_c.await.unwrap(), 7);
        assert_eq!(map.forget_session("s1"), 0);
    }
}
