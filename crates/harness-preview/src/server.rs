//! Spawning and supervising one dev-server process.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncRead;
use tokio::process::{Child, Command};

use crate::{sniff, PreviewError};

/// How long `start` waits for the server to accept connections by default.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(90);
/// How often the readiness loop retries the port / drains output.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Retained log lines (ring buffer) and per-line cap, so a chatty server
/// can't grow memory or blow up the model's context via `dev_server_logs`.
const MAX_LOG_LINES: usize = 400;
const MAX_LINE_CHARS: usize = 500;

/// Lifecycle of a dev server, mirrored to the host UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewPhase {
    /// Process spawned; waiting for it to accept connections.
    Starting,
    /// Reachable — the preview can load it.
    Ready,
    /// Exited unexpectedly or never became reachable.
    Error,
    /// Stopped on purpose (tool call, session end, replacement).
    Stopped,
}

/// A lifecycle snapshot, pushed to the host on every phase change and
/// available on demand via [`DevServer::status`].
#[derive(Debug, Clone, Serialize)]
pub struct PreviewStatus {
    pub phase: PreviewPhase,
    /// Short name of the server (e.g. "dev").
    pub name: String,
    /// The shell command the server was started with.
    pub command: String,
    /// Loadable URL, once known (always set when `phase == Ready`).
    pub url: Option<String>,
    pub port: Option<u16>,
    /// Human-readable detail for error/stopped phases.
    pub message: Option<String>,
}

/// A host surface that reacts to dev-server lifecycle changes — the desktop
/// opens/updates/closes the preview panel, the CLI paints a status line.
/// Notifications are fire-and-forget.
pub trait PreviewSink: Send + Sync {
    fn status(&self, status: &PreviewStatus);

    /// Project files changed under a server with no hot reload of its own —
    /// the host should refresh its preview surface. Default: nothing (the CLI
    /// has no embedded view; the user's browser owns refresh).
    fn reload_needed(&self) {}
}

/// What to run and where it should listen.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub name: String,
    pub command: String,
    /// Exact port the command listens on, when the caller knows it.
    pub port: Option<u16>,
    /// When no `port` is given, pick a free one and export it as `PORT`.
    /// The printed URL still wins if the framework ignores the variable.
    pub auto_port: bool,
}

#[derive(Debug, Clone)]
struct State {
    phase: PreviewPhase,
    port: Option<u16>,
    url: Option<String>,
    message: Option<String>,
}

/// One running (or finished) dev-server process.
pub struct DevServer {
    spec: ServerSpec,
    root: PathBuf,
    /// Pid at spawn time — kept separately because `Child::id()` is `None`
    /// after exit, and it doubles as the process-group id to kill.
    pid: Option<u32>,
    child: Mutex<Option<Child>>,
    logs: Arc<Mutex<VecDeque<String>>>,
    state: RwLock<State>,
    sink: Arc<dyn PreviewSink>,
    /// Reload-on-change watcher for servers without their own hot reload
    /// (see [`crate::watch`]); dropping it ends the watch.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Whether the process group was already SIGKILLed. Kill exactly once:
    /// pids (and thus group ids) are recycled, so a second kill much later —
    /// e.g. `Drop` on a server whose child crashed hours ago — could target
    /// an unrelated process tree.
    group_killed: std::sync::atomic::AtomicBool,
}

impl DevServer {
    /// Spawn `spec.command` in `root` and wait (up to `ready_timeout`) for the
    /// server to accept TCP connections. The listening port is the sniffed one
    /// from the server's own output when it prints a local URL, else the
    /// assigned/declared port. On failure the process group is killed and the
    /// error carries a tail of the output.
    pub async fn start(
        spec: ServerSpec,
        root: &Path,
        sink: Arc<dyn PreviewSink>,
        ready_timeout: Duration,
    ) -> Result<Arc<Self>, PreviewError> {
        // A declared port that's already serving belongs to someone else; the
        // child would die with EADDRINUSE while the readiness probe happily
        // connected to the squatter and attested a foreign app as "Ready".
        if let Some(port) = spec.port {
            if port_accepts(port).await {
                return Err(PreviewError::Server(format!(
                    "port {port} is already in use by another process — stop \
                     that process, pick a different port, or omit `port` to \
                     auto-assign one"
                )));
            }
        }
        let assigned_port = match (spec.port, spec.auto_port) {
            (Some(port), _) => Some(port),
            (None, true) => Some(find_free_port()?),
            (None, false) => None,
        };

        let mut command = shell_command(&spec.command);
        command
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(port) = assigned_port {
            command.env("PORT", port.to_string());
        }
        // Piped stdout makes Python block-buffer its "Serving HTTP on …" line,
        // which would blind the URL sniffer (a bare `python3 -m http.server`
        // ignores PORT and prints its real port instead). Harmless otherwise.
        command.env("PYTHONUNBUFFERED", "1");
        // Own process group so stop() can kill the whole `sh → npm → node`
        // tree, not just the shell.
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command
            .spawn()
            .map_err(|e| PreviewError::Server(format!("spawning `{}`: {e}", spec.command)))?;
        let pid = child.id();

        let logs = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        if let Some(out) = child.stdout.take() {
            pump_lines(out, logs.clone(), tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            pump_lines(err, logs.clone(), tx.clone());
        }
        drop(tx);

        let server = Arc::new(Self {
            spec,
            root: root.to_path_buf(),
            pid,
            child: Mutex::new(None),
            logs,
            state: RwLock::new(State {
                phase: PreviewPhase::Starting,
                port: assigned_port,
                url: None,
                message: None,
            }),
            sink,
            watcher: Mutex::new(None),
            group_killed: std::sync::atomic::AtomicBool::new(false),
        });
        server.emit();

        // Readiness: drain output collecting printed local URLs (the source
        // of truth for the port — frameworks ignore $PORT), and poll every
        // candidate port until one accepts while the child is still alive.
        // Multiple candidates matter: some servers print a debugger or proxy
        // URL before the real one.
        let deadline = tokio::time::Instant::now() + ready_timeout;
        let mut sniffed: Vec<(u16, String)> = Vec::new();
        loop {
            while let Ok(line) = rx.try_recv() {
                if let Some((port, url)) = sniff::detect_local_url(&line) {
                    if !sniffed.iter().any(|(p, _)| *p == port) {
                        sniffed.push((port, url));
                    }
                }
            }
            if let Ok(Some(status)) = child.try_wait() {
                // The leader exited, but a daemonizing command (`… &`) may
                // have left grandchildren in the group — take them down too,
                // or they'd leak with no handle left to clean them.
                server.kill_group_once();
                let tail = server.logs_tail(30);
                server.fail(format!("exited immediately ({status})"));
                return Err(PreviewError::Server(format!(
                    "`{}` exited before serving ({status}). The server must \
                     run in the foreground (don't background it with `&`). \
                     Output:\n{tail}",
                    server.spec.command
                )));
            }
            let mut candidates: Vec<(u16, Option<String>)> =
                sniffed.iter().map(|(p, u)| (*p, Some(u.clone()))).collect();
            if let Some(port) = assigned_port {
                if !candidates.iter().any(|(p, _)| *p == port) {
                    candidates.push((port, None));
                }
            }
            for (port, sniffed_url) in candidates {
                // Accepting connections only counts while our child is alive —
                // otherwise a foreign process on the same port gets attested.
                if port_accepts(port).await && matches!(child.try_wait(), Ok(None)) {
                    let url = sniffed_url.unwrap_or_else(|| format!("http://localhost:{port}"));
                    // Install the child before announcing Ready, so a stop()
                    // racing the announcement finds a process to kill.
                    *server.child.lock().unwrap() = Some(child);
                    {
                        let mut state = server.state.write().unwrap();
                        state.phase = PreviewPhase::Ready;
                        state.port = Some(port);
                        state.url = Some(url);
                    }
                    server.emit();
                    spawn_watchdog(Arc::downgrade(&server));
                    // HMR frameworks refresh the browser themselves; anything
                    // else gets a workspace watch that reloads the preview
                    // after each edit batch.
                    if !crate::watch::hmr_capable(&server.root, &server.spec.command) {
                        let sink = server.sink.clone();
                        match crate::watch::spawn(&server.root, move || sink.reload_needed()) {
                            Ok(watcher) => *server.watcher.lock().unwrap() = Some(watcher),
                            Err(e) => tracing::warn!("preview reload watch failed: {e}"),
                        }
                    }
                    return Ok(server);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                server.kill_group_once();
                let _ = child.start_kill();
                let tail = server.logs_tail(30);
                server.fail(format!("not reachable after {}s", ready_timeout.as_secs()));
                return Err(PreviewError::Server(format!(
                    "`{}` did not accept connections within {}s. Output:\n{tail}",
                    server.spec.command,
                    ready_timeout.as_secs()
                )));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Current lifecycle snapshot.
    pub fn status(&self) -> PreviewStatus {
        let state = self.state.read().unwrap().clone();
        PreviewStatus {
            phase: state.phase,
            name: self.spec.name.clone(),
            command: self.spec.command.clone(),
            url: state.url,
            port: state.port,
            message: state.message,
        }
    }

    pub fn spec(&self) -> &ServerSpec {
        &self.spec
    }

    /// The workspace the server runs in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The last `n` lines of merged stdout/stderr.
    pub fn logs_tail(&self, n: usize) -> String {
        let logs = self.logs.lock().unwrap();
        let skip = logs.len().saturating_sub(n);
        logs.iter()
            .skip(skip)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Kill the server (whole process group) and report `Stopped`.
    pub async fn stop(&self) {
        self.watcher.lock().unwrap().take();
        let child = self.child.lock().unwrap().take();
        self.kill_group_once();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let already_stopped = {
            let mut state = self.state.write().unwrap();
            let done = matches!(state.phase, PreviewPhase::Stopped | PreviewPhase::Error);
            if !done {
                state.phase = PreviewPhase::Stopped;
                state.message = Some("stopped".into());
            }
            done
        };
        if !already_stopped {
            self.emit();
        }
    }

    /// Whether the process is still supervised (spawned and not yet reaped).
    pub fn is_running(&self) -> bool {
        matches!(
            self.state.read().unwrap().phase,
            PreviewPhase::Starting | PreviewPhase::Ready
        )
    }

    fn fail(&self, message: String) {
        {
            let mut state = self.state.write().unwrap();
            // A deliberate stop is final — a watchdog observing the exit we
            // caused must not flip the terminal state to Error.
            if state.phase == PreviewPhase::Stopped {
                return;
            }
            state.phase = PreviewPhase::Error;
            state.message = Some(message);
        }
        self.emit();
    }

    fn emit(&self) {
        self.sink.status(&self.status());
    }

    /// SIGKILL the process group, at most once for this server's lifetime —
    /// group ids are recycled, so a late second kill could hit strangers.
    fn kill_group_once(&self) {
        use std::sync::atomic::Ordering;
        if !self.group_killed.swap(true, Ordering::SeqCst) {
            kill_group(self.pid);
        }
    }
}

impl std::fmt::Debug for DevServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevServer")
            .field("spec", &self.spec)
            .field("state", &*self.state.read().unwrap())
            .finish_non_exhaustive()
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        // Best-effort: never leak a server past its session. kill_on_drop
        // covers the shell; the group kill covers its children.
        self.kill_group_once();
        if let Some(mut child) = self.child.get_mut().map(Option::take).unwrap_or(None) {
            let _ = child.start_kill();
        }
    }
}

/// Watch for the process exiting on its own (crash, manual Ctrl-C in a
/// terminal, oom, …) and surface that to the UI. Holds only a `Weak` so a
/// dropped server doesn't linger just to be watched.
fn spawn_watchdog(server: Weak<DevServer>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(server) = server.upgrade() else {
                return;
            };
            let exited = {
                let mut child = server.child.lock().unwrap();
                match child.as_mut().map(|c| c.try_wait()) {
                    Some(Ok(Some(status))) => {
                        *child = None;
                        Some(status)
                    }
                    Some(Ok(None)) => None,
                    // Child gone (stopped elsewhere) or wait failed — done.
                    Some(Err(_)) | None => return,
                }
            };
            if let Some(status) = exited {
                // The leader died on its own; sweep any group survivors now,
                // while the group id is certainly still ours.
                server.kill_group_once();
                server.fail(format!("server exited ({status})"));
                return;
            }
        }
    });
}

/// Append lines from a child stream into the bounded ring buffer, and forward
/// them (best-effort) to the startup sniffer while someone is listening.
///
/// Splits on `\n` *and* `\r` (progress bars redraw with bare carriage
/// returns), and flushes an over-long "line" rather than buffering it — a
/// stream that never emits a newline must not grow memory for the server's
/// lifetime.
fn pump_lines<R>(
    reader: R,
    logs: Arc<Mutex<VecDeque<String>>>,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncReadExt;

    let emit = move |raw: &str,
                     logs: &Mutex<VecDeque<String>>,
                     tx: &tokio::sync::mpsc::UnboundedSender<String>| {
        let mut line = sniff::strip_ansi(raw);
        if line.trim().is_empty() {
            return;
        }
        if line.chars().count() > MAX_LINE_CHARS {
            line = line.chars().take(MAX_LINE_CHARS).collect::<String>() + " …";
        }
        {
            let mut logs = logs.lock().unwrap();
            if logs.len() == MAX_LOG_LINES {
                logs.pop_front();
            }
            logs.push_back(line.clone());
        }
        let _ = tx.send(line);
    };

    tokio::spawn(async move {
        let mut reader = reader;
        let mut chunk = [0u8; 8192];
        let mut pending = String::new();
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    while let Some(idx) = pending.find(['\n', '\r']) {
                        let line: String = pending.drain(..=idx).collect();
                        emit(line.trim_end_matches(['\n', '\r']), &logs, &tx);
                    }
                    if pending.chars().count() > MAX_LINE_CHARS {
                        emit(&std::mem::take(&mut pending), &logs, &tx);
                    }
                }
            }
        }
        if !pending.is_empty() {
            emit(&pending, &logs, &tx);
        }
    });
}

/// Whether something is accepting TCP connections on localhost:`port`.
async fn port_accepts(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await,
        Ok(Ok(_))
    )
}

fn find_free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Kill the process group rooted at `pid` (Unix). The group exists because the
/// child was spawned with `process_group(0)`.
#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    // SAFETY: plain syscall with no memory concerns; a negative pid targets
    // the process group we created at spawn, so we can't signal unrelated
    // processes.
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {
    // Windows: kill_on_drop / start_kill handles the direct child; grandchild
    // cleanup needs a job object and is left for the Windows pass.
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sink that records every status it sees.
    pub(crate) struct RecordingSink(pub Mutex<Vec<PreviewStatus>>);

    impl RecordingSink {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(Vec::new())))
        }
        pub(crate) fn phases(&self) -> Vec<PreviewPhase> {
            self.0.lock().unwrap().iter().map(|s| s.phase).collect()
        }
    }

    impl PreviewSink for RecordingSink {
        fn status(&self, status: &PreviewStatus) {
            self.0.lock().unwrap().push(status.clone());
        }
    }

    pub(crate) fn python3() -> Option<&'static str> {
        // The http.server tests need a real server binary; skip gracefully on
        // machines without python3 rather than fail.
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok()
        {
            Some("python3")
        } else {
            None
        }
    }

    fn spec(command: &str) -> ServerSpec {
        ServerSpec {
            name: "dev".into(),
            command: command.into(),
            port: None,
            auto_port: true,
        }
    }

    #[tokio::test]
    async fn starts_serves_and_stops_a_real_server() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<h1>ox</h1>").unwrap();
        let sink = RecordingSink::new();

        let server = DevServer::start(
            spec(&format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1")),
            dir.path(),
            sink.clone(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        let status = server.status();
        assert_eq!(status.phase, PreviewPhase::Ready);
        let port = status.port.unwrap();
        assert!(status.url.as_deref().unwrap().contains(&port.to_string()));
        assert!(port_accepts(port).await);

        server.stop().await;
        assert_eq!(server.status().phase, PreviewPhase::Stopped);
        // The group kill must take the python process down with the shell.
        for _ in 0..50 {
            if !port_accepts(port).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !port_accepts(port).await,
            "server still listening after stop"
        );
        assert_eq!(
            sink.phases(),
            vec![
                PreviewPhase::Starting,
                PreviewPhase::Ready,
                PreviewPhase::Stopped
            ]
        );
    }

    #[tokio::test]
    async fn sniffs_the_real_port_when_the_command_ignores_assigned_port() {
        // The live failure this guards: a python http.server that ignores the
        // assigned $PORT (here: a hard-coded port, like the bare default-8000
        // invocation) and announces its real URL on stdout — which Python
        // block-buffers under a pipe unless PYTHONUNBUFFERED is set. Readiness
        // must come from sniffing that announcement, not the assigned port.
        let Some(py) = python3() else { return };
        let real_port = find_free_port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let server = DevServer::start(
            spec(&format!("{py} -m http.server {real_port} --bind 127.0.0.1")),
            dir.path(),
            RecordingSink::new(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        let status = server.status();
        assert_eq!(status.port, Some(real_port), "sniffed port: {status:?}");
        server.stop().await;
    }

    #[tokio::test]
    async fn refuses_a_declared_port_someone_else_is_serving() {
        // Without the pre-spawn check, the child dies with EADDRINUSE while
        // the readiness probe connects to the squatter — attesting a foreign
        // app as "your app, ready".
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let squatter = DevServer::start(
            spec(&format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1")),
            dir.path(),
            RecordingSink::new(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        let taken = squatter.status().port.unwrap();

        let err = DevServer::start(
            ServerSpec {
                name: "dev".into(),
                command: format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1"),
                port: Some(taken),
                auto_port: false,
            },
            dir.path(),
            RecordingSink::new(),
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already in use"), "err: {err}");
        squatter.stop().await;
    }

    #[tokio::test]
    async fn a_backgrounded_server_is_rejected_and_not_left_running() {
        // `… &` makes the shell exit 0 immediately while the real server keeps
        // serving in the same process group — the classic leak. It must be
        // reported as a failure AND killed.
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let port = find_free_port().unwrap();
        let err = DevServer::start(
            ServerSpec {
                name: "dev".into(),
                command: format!("{py} -m http.server {port} --bind 127.0.0.1 &"),
                port: None,
                auto_port: false,
            },
            dir.path(),
            RecordingSink::new(),
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("exited before serving"),
            "err: {err}"
        );
        // The grandchild must not survive the failed start.
        for _ in 0..50 {
            if !port_accepts(port).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !port_accepts(port).await,
            "a backgrounded server survived a failed start"
        );
    }

    #[tokio::test]
    async fn early_exit_reports_error_with_output() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RecordingSink::new();
        let err = DevServer::start(
            spec("echo boom-nope && exit 3"),
            dir.path(),
            sink.clone(),
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("boom-nope"), "missing output tail: {msg}");
        assert!(sink.phases().contains(&PreviewPhase::Error));
    }

    #[tokio::test]
    async fn unreachable_server_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RecordingSink::new();
        let err = DevServer::start(
            spec("sleep 60"),
            dir.path(),
            sink.clone(),
            Duration::from_millis(700),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("did not accept connections"));
    }

    #[tokio::test]
    async fn watchdog_notices_a_crash() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let sink = RecordingSink::new();
        // Serve, then die shortly after readiness.
        let server = DevServer::start(
            spec(&format!(
                "{py} -m http.server \"$PORT\" --bind 127.0.0.1 & pid=$!; sleep 1; kill $pid; wait"
            )),
            dir.path(),
            sink.clone(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert_eq!(server.status().phase, PreviewPhase::Ready);
        for _ in 0..80 {
            if server.status().phase == PreviewPhase::Error {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(server.status().phase, PreviewPhase::Error);
    }
}
