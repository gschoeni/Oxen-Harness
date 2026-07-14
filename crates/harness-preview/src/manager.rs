//! At most one dev server per chat session, shared between the tools (which
//! start/stop servers) and the host UI (which lists them and shows previews).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::server::{DevServer, PreviewSink, PreviewStatus, ServerSpec};
use crate::PreviewError;

/// Session-keyed registry of dev servers. Cloning is cheap and shares state,
/// so a host keeps one manager and hands clones to each session's tools.
#[derive(Clone, Default)]
pub struct DevServerManager {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    servers: Mutex<HashMap<String, Arc<DevServer>>>,
    /// Serializes `start` per session — starting takes up to the readiness
    /// timeout, and two concurrent starts for one session (fleet subagents
    /// share their parent's session key) must not race the remove/insert
    /// around that long await.
    starting: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Set by [`DevServerManager::stop_all`] (app shutdown): no server may be
    /// started or re-inserted afterwards, so an in-flight `start` can't leak a
    /// process past the shutdown sweep.
    closed: AtomicBool,
}

impl DevServerManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a server for `session`, replacing (and stopping) any existing
    /// one — one server per session. Concurrent starts for the same session
    /// are serialized: the later caller waits, then replaces the earlier
    /// caller's server.
    pub async fn start(
        &self,
        session: &str,
        spec: ServerSpec,
        root: &Path,
        sink: Arc<dyn PreviewSink>,
        ready_timeout: Duration,
    ) -> Result<Arc<DevServer>, PreviewError> {
        let session_lock = {
            let mut starting = self.inner.starting.lock().unwrap();
            starting.entry(session.to_string()).or_default().clone()
        };
        let _guard = session_lock.lock().await;

        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(PreviewError::Server("the app is shutting down".into()));
        }
        let previous = self.inner.servers.lock().unwrap().remove(session);
        if let Some(previous) = previous {
            previous.stop().await;
        }
        let server = DevServer::start(spec, root, sink, ready_timeout).await?;
        // A shutdown that ran while we were starting has already swept the
        // map; don't re-insert a running server behind its back.
        if self.inner.closed.load(Ordering::SeqCst) {
            server.stop().await;
            return Err(PreviewError::Server("the app is shutting down".into()));
        }
        self.inner
            .servers
            .lock()
            .unwrap()
            .insert(session.to_string(), server.clone());
        Ok(server)
    }

    /// The session's server (running or not), if one was started.
    pub fn get(&self, session: &str) -> Option<Arc<DevServer>> {
        self.inner.servers.lock().unwrap().get(session).cloned()
    }

    /// Stop and forget the session's server. Returns whether one existed.
    pub async fn stop(&self, session: &str) -> bool {
        let server = self.inner.servers.lock().unwrap().remove(session);
        match server {
            Some(server) => {
                server.stop().await;
                true
            }
            None => false,
        }
    }

    /// Stop every server and refuse any further starts (app shutdown).
    pub async fn stop_all(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        let servers: Vec<_> = self.inner.servers.lock().unwrap().drain().collect();
        for (_, server) in servers {
            server.stop().await;
        }
    }

    /// Status of every known server, keyed by session — for settings /
    /// sidebar UI.
    pub fn statuses(&self) -> Vec<(String, PreviewStatus)> {
        self.inner
            .servers
            .lock()
            .unwrap()
            .iter()
            .map(|(session, server)| (session.clone(), server.status()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{python3, RecordingSink};
    use crate::server::PreviewPhase;

    fn spec(command: &str) -> ServerSpec {
        ServerSpec {
            name: "dev".into(),
            command: command.into(),
            port: None,
            auto_port: true,
        }
    }

    #[tokio::test]
    async fn one_server_per_session_replaces_previous() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let manager = DevServerManager::new();
        let serve = format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1");

        let first = manager
            .start(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        let first_port = first.status().port.unwrap();

        let second = manager
            .start(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
            )
            .await
            .unwrap();

        assert_eq!(first.status().phase, PreviewPhase::Stopped);
        assert_eq!(second.status().phase, PreviewPhase::Ready);
        assert_ne!(second.status().port.unwrap(), first_port);
        assert_eq!(manager.statuses().len(), 1);

        assert!(manager.stop("s1").await);
        assert!(!manager.stop("s1").await);
        assert!(manager.get("s1").is_none());
    }

    #[tokio::test]
    async fn concurrent_starts_for_one_session_leave_exactly_one_server() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let manager = DevServerManager::new();
        let serve = format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1");

        // Fleet subagents share their parent's session key: racing starts
        // must serialize, ending with exactly one live server.
        let (a, b) = tokio::join!(
            manager.start(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
            ),
            manager.start(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
            ),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        let live = [&a, &b]
            .iter()
            .filter(|s| s.status().phase == PreviewPhase::Ready)
            .count();
        assert_eq!(live, 1, "exactly one of the racing starts survives");
        assert_eq!(manager.statuses().len(), 1);
        manager.stop_all().await;
    }

    #[tokio::test]
    async fn no_starts_after_stop_all() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let manager = DevServerManager::new();
        manager.stop_all().await;

        let err = manager
            .start(
                "s1",
                spec(&format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1")),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("shutting down"));
        assert!(manager.get("s1").is_none());
    }
}
