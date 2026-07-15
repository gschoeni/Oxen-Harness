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

/// How [`DevServerManager::start_or_reuse`] satisfied a start request — the
/// tool words its result to the model differently for each, so "already
/// running" never reads as "freshly started".
pub enum StartOutcome {
    /// A new process was spawned (any previous server for the session was
    /// stopped first).
    Started(Arc<DevServer>),
    /// The session's own server was already running this command — handed
    /// back untouched.
    Reused(Arc<DevServer>),
    /// Another session's server in the same workspace was already running
    /// this command; it now belongs to the requesting session (its lifecycle
    /// events re-point at the new session's sink).
    Adopted {
        server: Arc<DevServer>,
        from_session: String,
    },
}

impl StartOutcome {
    pub fn server(&self) -> &Arc<DevServer> {
        match self {
            StartOutcome::Started(s) | StartOutcome::Reused(s) => s,
            StartOutcome::Adopted { server, .. } => server,
        }
    }
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
        let session_lock = self.session_lock(session);
        let _guard = session_lock.lock().await;
        self.start_locked(session, spec, root, sink, ready_timeout)
            .await
    }

    /// Satisfy a start request without needlessly spawning a duplicate:
    ///
    /// 1. The session's own server already runs this command and is serving →
    ///    **reuse** it (nothing restarts, the port stays put).
    /// 2. Another session's server in the same workspace runs this command →
    ///    **adopt** it: re-key it to this session and point its events at
    ///    this session's sink (the old session's UI is told it moved).
    /// 3. Otherwise — different command, dead server, or `force_restart` —
    ///    replace-and-start like [`DevServerManager::start`].
    ///
    /// The matching rule is deliberately strict (same trimmed command, and the
    /// declared port if any): two *different* commands in one workspace are a
    /// legitimate pair (frontend + api), never merged.
    pub async fn start_or_reuse(
        &self,
        session: &str,
        spec: ServerSpec,
        root: &Path,
        sink: Arc<dyn PreviewSink>,
        ready_timeout: Duration,
        force_restart: bool,
    ) -> Result<StartOutcome, PreviewError> {
        let session_lock = self.session_lock(session);
        let _guard = session_lock.lock().await;

        if !force_restart {
            if let Some(existing) = self.get(session) {
                if serves_spec(&existing, &spec) {
                    return Ok(StartOutcome::Reused(existing));
                }
            }
            // Find-and-move under one lock so a racing start can't adopt the
            // same server twice.
            let adopted = {
                let mut servers = self.inner.servers.lock().unwrap();
                let found = servers
                    .iter()
                    .find(|(owner, server)| {
                        owner.as_str() != session
                            && server.root() == root
                            && serves_spec(server, &spec)
                    })
                    .map(|(owner, server)| (owner.clone(), server.clone()));
                found.map(|(owner, server)| {
                    servers.remove(&owner);
                    let displaced = servers.remove(session);
                    servers.insert(session.to_string(), server.clone());
                    (owner, server, displaced)
                })
            };
            if let Some((from_session, server, displaced)) = adopted {
                // This session's own (non-matching or dead) server gives way.
                if let Some(displaced) = displaced {
                    displaced.stop().await;
                }
                // New sink first (announces Ready to the adopting session's
                // UI), then tell the old session's UI its preview moved on.
                let old_sink = server.replace_sink(sink);
                let mut moved = server.status();
                moved.phase = crate::PreviewPhase::Stopped;
                moved.message = Some("the preview was adopted by another chat".into());
                old_sink.status(&moved);
                return Ok(StartOutcome::Adopted {
                    server,
                    from_session,
                });
            }
        }

        self.start_locked(session, spec, root, sink, ready_timeout)
            .await
            .map(StartOutcome::Started)
    }

    fn session_lock(&self, session: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut starting = self.inner.starting.lock().unwrap();
        starting.entry(session.to_string()).or_default().clone()
    }

    /// The replace-and-start body; callers hold the session's start lock.
    async fn start_locked(
        &self,
        session: &str,
        spec: ServerSpec,
        root: &Path,
        sink: Arc<dyn PreviewSink>,
        ready_timeout: Duration,
    ) -> Result<Arc<DevServer>, PreviewError> {
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

/// Whether `server` is currently serving exactly what `spec` asks for: it is
/// `Ready` and runs the same (trimmed) command — and the same port, when the
/// request declares one. Display `name` differences don't matter.
fn serves_spec(server: &DevServer, spec: &ServerSpec) -> bool {
    let status = server.status();
    status.phase == crate::PreviewPhase::Ready
        && status.command.trim() == spec.command.trim()
        && spec.port.is_none_or(|p| status.port == Some(p))
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
    async fn start_or_reuse_hands_back_the_running_server() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let manager = DevServerManager::new();
        let serve = format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1");

        let first = match manager
            .start_or_reuse(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
                false,
            )
            .await
            .unwrap()
        {
            StartOutcome::Started(s) => s,
            other => panic!("expected Started, got {}", outcome_name(&other)),
        };
        let port = first.status().port.unwrap();

        // Same command, still serving → reused untouched.
        let outcome = manager
            .start_or_reuse(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
                false,
            )
            .await
            .unwrap();
        match &outcome {
            StartOutcome::Reused(s) => {
                assert_eq!(s.status().port, Some(port), "reuse must not churn the port")
            }
            other => panic!("expected Reused, got {}", outcome_name(other)),
        }
        assert_eq!(manager.statuses().len(), 1);

        // A different command replaces; force_restart replaces even when equal.
        let outcome = manager
            .start_or_reuse(
                "s1",
                spec(&serve),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
                true,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, StartOutcome::Started(_)));
        assert_eq!(first.status().phase, PreviewPhase::Stopped);
        manager.stop_all().await;
    }

    #[tokio::test]
    async fn a_matching_server_from_another_session_is_adopted_not_duplicated() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let manager = DevServerManager::new();
        let serve = format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1");
        let old_sink = RecordingSink::new();

        let original = manager
            .start(
                "old-chat",
                spec(&serve),
                dir.path(),
                old_sink.clone(),
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        let port = original.status().port.unwrap();

        // A new chat in the same workspace asking for the same command gets
        // the running server, re-keyed — never a duplicate on another port.
        let new_sink = RecordingSink::new();
        let outcome = manager
            .start_or_reuse(
                "new-chat",
                spec(&serve),
                dir.path(),
                new_sink.clone(),
                Duration::from_secs(30),
                false,
            )
            .await
            .unwrap();
        match &outcome {
            StartOutcome::Adopted {
                server,
                from_session,
            } => {
                assert_eq!(from_session, "old-chat");
                assert_eq!(server.status().port, Some(port));
                assert_eq!(server.status().phase, PreviewPhase::Ready);
            }
            other => panic!("expected Adopted, got {}", outcome_name(other)),
        }
        assert_eq!(manager.statuses().len(), 1, "exactly one server remains");
        assert!(manager.get("old-chat").is_none());
        assert!(manager.get("new-chat").is_some());
        // The adopting session's UI saw it come up; the old one saw it move on.
        assert!(new_sink.phases().contains(&PreviewPhase::Ready));
        assert_eq!(old_sink.phases().last(), Some(&PreviewPhase::Stopped));

        // A *different* command in the same workspace is a second server
        // (frontend + api is legitimate), not an adoption target.
        let outcome = manager
            .start_or_reuse(
                "api-chat",
                spec(&format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1 --directory .")),
                dir.path(),
                RecordingSink::new(),
                Duration::from_secs(30),
                false,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, StartOutcome::Started(_)));
        assert_eq!(manager.statuses().len(), 2);
        manager.stop_all().await;
    }

    fn outcome_name(outcome: &StartOutcome) -> &'static str {
        match outcome {
            StartOutcome::Started(_) => "Started",
            StartOutcome::Reused(_) => "Reused",
            StartOutcome::Adopted { .. } => "Adopted",
        }
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
