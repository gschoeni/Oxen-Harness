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

mod bridges;
mod service;
pub mod translate;

use harness_protocol::ProtocolEvent;

pub use bridges::{
    HostAsker, HostApprover, HostCanvasSink, HostFleetSink, HostViewerSink, NoScreenshotLens,
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
pub struct PendingMap<T> {
    inner: std::sync::Mutex<PendingEntries<T>>,
    counter: std::sync::atomic::AtomicU64,
}

type PendingEntries<T> = std::collections::HashMap<String, tokio::sync::oneshot::Sender<T>>;

impl<T> Default for PendingMap<T> {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
            counter: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl<T> PendingMap<T> {
    /// Park a new round-trip, returning its id and the receiver to await.
    pub fn register(&self, prefix: &str) -> (String, tokio::sync::oneshot::Receiver<T>) {
        let id = format!(
            "{prefix}{}",
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .lock()
            .expect("pending map poisoned")
            .insert(id.clone(), tx);
        (id, rx)
    }

    /// Deliver the client's answer to a parked round-trip. Returns false when
    /// the id is unknown (already answered, cancelled, or evicted) — ignored
    /// by design.
    pub fn deliver(&self, id: &str, value: T) -> bool {
        let sender = self.inner.lock().expect("pending map poisoned").remove(id);
        match sender {
            Some(tx) => tx.send(value).is_ok(),
            None => false,
        }
    }

    /// Drop a parked round-trip without answering (chat evicted, client gone):
    /// the waiting side sees a closed channel and treats it as "no answer".
    pub fn forget(&self, id: &str) {
        self.inner.lock().expect("pending map poisoned").remove(id);
    }
}

/// Questions awaiting a client answer (`ask_user_question`).
pub type PendingQuestions = std::sync::Arc<PendingMap<Vec<harness_tools::QuestionAnswer>>>;
/// Permission approvals awaiting a client decision.
pub type PendingApprovals = std::sync::Arc<PendingMap<harness_protocol::ApprovalAnswer>>;
