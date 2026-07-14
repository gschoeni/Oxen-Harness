//! The model-facing dev-server tools.
//!
//! One trio per session, all sharing the [`DevServerManager`]: start (blocks
//! until the server is reachable, then the host shows the live preview), stop,
//! and read logs. The host wires a [`PreviewSink`] so lifecycle changes drive
//! its preview UI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use harness_tools::{ToolError, TypedTool};
use serde::Deserialize;

use crate::config::{self, SavedServer};
use crate::server::{PreviewSink, ServerSpec, DEFAULT_READY_TIMEOUT};
use crate::DevServerManager;

pub const START_DEV_SERVER_TOOL: &str = "start_dev_server";
pub const STOP_DEV_SERVER_TOOL: &str = "stop_dev_server";
pub const DEV_SERVER_LOGS_TOOL: &str = "dev_server_logs";
pub const PREVIEW_SCREENSHOT_TOOL: &str = "preview_screenshot";
pub const PREVIEW_CONSOLE_TOOL: &str = "preview_console";

/// Longest readiness wait a tool call may request (10 minutes).
const MAX_WAIT_MS: u64 = 600_000;
const DEFAULT_LOG_LINES: usize = 60;

/// Everything the tools need to act for one session.
#[derive(Clone)]
struct SessionContext {
    manager: DevServerManager,
    session: String,
    root: PathBuf,
    sink: Arc<dyn PreviewSink>,
}

/// How the agent *sees* the preview — implemented by hosts with an embedded
/// view (the desktop's native webview). Hosts without one (the CLI) simply
/// don't register the sight tools.
#[async_trait]
pub trait PreviewLens: Send + Sync {
    /// Capture the current preview as PNG bytes.
    async fn screenshot(&self) -> Result<Vec<u8>, String>;
    /// The last `n` captured browser console lines for this session.
    fn console_tail(&self, n: usize) -> Vec<crate::console::ConsoleLine>;
}

/// Start (or restart) the session's dev server and wait until it serves.
pub struct StartDevServerTool {
    ctx: SessionContext,
    /// Host-specific sentence appended to a successful start, telling the
    /// model how to verify its work (only hosts with sight tools set one).
    verify_hint: Option<String>,
}

impl StartDevServerTool {
    /// Append `hint` to every successful start result.
    pub fn with_verify_hint(mut self, hint: impl Into<String>) -> Self {
        self.verify_hint = Some(hint.into());
        self
    }
}

/// Stop the session's dev server.
pub struct StopDevServerTool {
    ctx: SessionContext,
}

/// Read the dev server's recent output.
pub struct DevServerLogsTool {
    ctx: SessionContext,
}

/// Build the session's tool trio.
pub fn session_tools(
    manager: DevServerManager,
    session: impl Into<String>,
    root: impl Into<PathBuf>,
    sink: Arc<dyn PreviewSink>,
) -> (StartDevServerTool, StopDevServerTool, DevServerLogsTool) {
    let ctx = SessionContext {
        manager,
        session: session.into(),
        root: root.into(),
        sink,
    };
    (
        StartDevServerTool {
            ctx: ctx.clone(),
            verify_hint: None,
        },
        StopDevServerTool { ctx: ctx.clone() },
        DevServerLogsTool { ctx },
    )
}

/// Build the sight pair (screenshot + console) for hosts with an embedded
/// preview. Register alongside [`session_tools`].
pub fn sight_tools(
    manager: DevServerManager,
    session: impl Into<String>,
    lens: Arc<dyn PreviewLens>,
) -> (PreviewScreenshotTool, PreviewConsoleTool) {
    let session = session.into();
    (
        PreviewScreenshotTool {
            manager: manager.clone(),
            session: session.clone(),
            lens: lens.clone(),
        },
        PreviewConsoleTool { lens },
    )
}

/// Arguments to `start_dev_server`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct StartDevServerArgs {
    /// Shell command that starts the server. Runs from the workspace root. A
    /// free port is exported as $PORT — pass it explicitly when the command
    /// accepts one (e.g. "python3 -m http.server \"$PORT\""); commands that
    /// pick their own port (vite, next) are fine as-is, the real port is
    /// detected from their output.
    pub command: String,
    /// The exact port the command listens on, ONLY if it is fixed (e.g. an
    /// OAuth callback). Omit it normally: a free port is chosen, exported as
    /// the PORT environment variable, and the real port is auto-detected from
    /// the server's output even if the command ignores PORT.
    pub port: Option<u16>,
    /// Short display name (default "dev"); use distinct names like
    /// "frontend"/"api" if the project has several entry points.
    pub name: Option<String>,
    /// How long to wait for the server to accept connections, in milliseconds
    /// (default 90000).
    pub wait_timeout_ms: Option<u64>,
}

#[async_trait]
impl TypedTool for StartDevServerTool {
    const NAME: &'static str = START_DEV_SERVER_TOOL;
    type Args = StartDevServerArgs;

    fn description(&self) -> &str {
        "Start the project's dev server as a supervised background process and \
         wait until it is reachable, then show the running app to the user in a \
         live preview panel. Use this — never `run_shell` — for anything \
         long-running that serves HTTP (vite, next, python http.server, …). \
         Call it as soon as there is something to look at, and again to restart \
         after changing server config. Each call replaces the session's \
         previous server. Returns the URL; the server keeps running in the \
         background (check output with dev_server_logs, stop with \
         stop_dev_server)."
    }

    async fn run(&self, args: StartDevServerArgs) -> Result<String, ToolError> {
        let command = args.command.trim().to_string();
        if command.is_empty() {
            return Err(ToolError::InvalidArguments(
                "missing non-empty `command`".into(),
            ));
        }
        let name = args
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("dev")
            .to_string();
        let timeout = Duration::from_millis(
            args.wait_timeout_ms
                .unwrap_or(DEFAULT_READY_TIMEOUT.as_millis() as u64)
                .min(MAX_WAIT_MS),
        );
        let spec = ServerSpec {
            name: name.clone(),
            command: command.clone(),
            port: args.port,
            auto_port: args.port.is_none(),
        };

        // Starting replaces (and first stops) any server this session had, so
        // say so on failure — otherwise the model reports "it's still running"
        // when nothing is.
        let replaced = self.ctx.manager.get(&self.ctx.session).is_some();
        let server = self
            .ctx
            .manager
            .start(
                &self.ctx.session,
                spec,
                &self.ctx.root,
                self.ctx.sink.clone(),
                timeout,
            )
            .await
            .map_err(|e| {
                ToolError::Execution(if replaced {
                    format!("{e}\n(The session's previous dev server was stopped to make way for this one — nothing is serving now.)")
                } else {
                    e.to_string()
                })
            })?;

        let status = server.status();
        let url = status.url.clone().unwrap_or_default();
        // Remember the working spec so future sessions can one-click start it.
        let _ = config::remember(
            &self.ctx.root,
            SavedServer {
                name,
                command,
                port: args.port,
                auto_port: args.port.is_none(),
            },
        );

        let mut message = format!(
            "Dev server \"{}\" is running at {url} (port {}). It keeps running \
             across turns — do NOT start it again unless it stopped or its \
             command changed.",
            status.name,
            status.port.unwrap_or_default(),
        );
        if let Some(hint) = &self.verify_hint {
            message.push(' ');
            message.push_str(hint);
        }
        message.push_str("\nRecent output:\n");
        message.push_str(&server.logs_tail(10));
        Ok(message)
    }
}

/// How long to let the page settle before capturing.
///
/// The agent's normal rhythm is *edit files, then look* — and at that moment
/// the browser is usually still catching up: HMR is applying the patch, or the
/// file watcher's debounced reload (300ms) hasn't even fired yet. Capturing
/// immediately would photograph the OLD page, and the model would confidently
/// report on a screen the user can no longer see. Waiting a beat is far
/// cheaper than a wrong conclusion.
const SCREENSHOT_SETTLE: Duration = Duration::from_millis(900);

/// Arguments to `preview_screenshot`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct PreviewScreenshotArgs {}

/// Capture what the user currently sees in the preview.
pub struct PreviewScreenshotTool {
    manager: DevServerManager,
    session: String,
    lens: Arc<dyn PreviewLens>,
}

#[async_trait]
impl TypedTool for PreviewScreenshotTool {
    const NAME: &'static str = PREVIEW_SCREENSHOT_TOOL;
    type Args = PreviewScreenshotArgs;

    fn description(&self) -> &str {
        "Capture a screenshot of the running app in the live preview and look \
         at it. Use this to verify your work after making visual changes — \
         check the layout, spot blank screens or error pages, and fix what you \
         see before telling the user it's done. Requires a running \
         start_dev_server preview."
    }

    async fn run(&self, _args: PreviewScreenshotArgs) -> Result<String, ToolError> {
        // A server that died mid-turn (a syntax error crashed vite) is the
        // common case here. Say so — with its dying words — instead of the
        // useless "no preview": this error is how the model finds out.
        let server = self.manager.get(&self.session);
        let status = server.as_ref().map(|s| s.status());
        let url = match &status {
            Some(s) if s.phase == crate::PreviewPhase::Ready => s.url.clone().unwrap_or_default(),
            Some(s) if s.phase == crate::PreviewPhase::Error => {
                let logs = server.as_ref().map(|s| s.logs_tail(20)).unwrap_or_default();
                return Err(ToolError::Execution(format!(
                    "the dev server is not running — it stopped with: {}\n\
                     Fix the problem, then restart it with start_dev_server. \
                     Its last output:\n{logs}",
                    s.message.as_deref().unwrap_or("an error")
                )));
            }
            _ => {
                return Err(ToolError::Execution(
                    "no running preview — start the app with start_dev_server first".into(),
                ))
            }
        };
        // Let a just-edited page finish reloading/hot-patching before we look
        // at it (see SCREENSHOT_SETTLE).
        tokio::time::sleep(SCREENSHOT_SETTLE).await;
        let png = self.lens.screenshot().await.map_err(ToolError::Execution)?;
        let path = write_screenshot(&self.session, &png)?;

        Ok(format!(
            "Screenshot of the running app at {url}: {}",
            harness_core::attach::image_marker(&path.display().to_string())
        ))
    }
}

/// Write a screenshot into a private, per-user directory and return its path.
///
/// Deliberately NOT a predictable name in the shared temp root: `/tmp` is
/// world-writable on Linux, and `fs::write` follows symlinks, so a planted
/// `oxen-preview-<session>-0.png -> ~/.ssh/authorized_keys` would let a local
/// attacker have us clobber an arbitrary file. Each session keeps only the
/// latest screenshot (the previous one is already in the transcript).
fn write_screenshot(session: &str, png: &[u8]) -> Result<PathBuf, ToolError> {
    let dir = screenshot_dir()?;
    // Session ids are UUIDs; keep the filename to that shape regardless so a
    // hostile session name can't escape the directory.
    let safe: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    let path = dir.join(format!("{safe}.png"));
    // Truncate-in-place on a path we own; the dir is 0700 and freshly created.
    std::fs::write(&path, png)?;
    Ok(path)
}

/// `<cache>/oxen-harness/previews/`, created 0700 on Unix so no other local
/// user can pre-plant symlinks in it.
fn screenshot_dir() -> Result<PathBuf, ToolError> {
    let base = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("oxen-harness")
        .join("previews");
    std::fs::create_dir_all(&base)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700));
    }
    Ok(base)
}

/// Arguments to `preview_console`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct PreviewConsoleArgs {
    /// How many trailing lines to return (default 40, max 200).
    pub lines: Option<usize>,
}

/// Read the preview page's captured console errors/warnings.
pub struct PreviewConsoleTool {
    lens: Arc<dyn PreviewLens>,
}

#[async_trait]
impl TypedTool for PreviewConsoleTool {
    const NAME: &'static str = PREVIEW_CONSOLE_TOOL;
    type Args = PreviewConsoleArgs;

    fn description(&self) -> &str {
        "Read the browser console errors and warnings captured from the live \
         preview page. Check this after exercising the app or when the preview \
         looks broken — a blank screen usually left an error here."
    }

    async fn run(&self, args: PreviewConsoleArgs) -> Result<String, ToolError> {
        let lines = self
            .lens
            .console_tail(args.lines.unwrap_or(40).clamp(1, 200));
        if lines.is_empty() {
            return Ok("No browser console errors or warnings captured.".into());
        }
        Ok(lines
            .iter()
            .map(|l| format!("[{}] {}", l.level, l.text))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// Arguments to `stop_dev_server`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct StopDevServerArgs {}

#[async_trait]
impl TypedTool for StopDevServerTool {
    const NAME: &'static str = STOP_DEV_SERVER_TOOL;
    type Args = StopDevServerArgs;

    fn description(&self) -> &str {
        "Stop this session's running dev server (started with start_dev_server) \
         and close the live preview. Use when the user is done, or before \
         switching the project to a different server setup."
    }

    async fn run(&self, _args: StopDevServerArgs) -> Result<String, ToolError> {
        if self.ctx.manager.stop(&self.ctx.session).await {
            Ok("Dev server stopped.".into())
        } else {
            Ok("No dev server is running for this session.".into())
        }
    }
}

/// Arguments to `dev_server_logs`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct DevServerLogsArgs {
    /// How many trailing lines to return (default 60, max 400).
    pub lines: Option<usize>,
}

#[async_trait]
impl TypedTool for DevServerLogsTool {
    const NAME: &'static str = DEV_SERVER_LOGS_TOOL;
    type Args = DevServerLogsArgs;

    fn description(&self) -> &str {
        "Read the dev server's recent output (merged stdout/stderr) and its \
         current status. Check this when the preview looks wrong, after \
         exercising the app, or to see request/error logs."
    }

    async fn run(&self, args: DevServerLogsArgs) -> Result<String, ToolError> {
        let Some(server) = self.ctx.manager.get(&self.ctx.session) else {
            return Ok(
                "No dev server has been started for this session. Use start_dev_server.".into(),
            );
        };
        let lines = args.lines.unwrap_or(DEFAULT_LOG_LINES).clamp(1, 400);
        let status = server.status();
        let header = match (&status.url, &status.message) {
            (Some(url), _) => format!("status: {:?} — {url}", status.phase),
            (None, Some(msg)) => format!("status: {:?} — {msg}", status.phase),
            (None, None) => format!("status: {:?}", status.phase),
        };
        let tail = server.logs_tail(lines);
        let body = if tail.is_empty() {
            "(no output)"
        } else {
            &tail
        };
        Ok(format!("{header}\n--- last {lines} lines ---\n{body}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{python3, RecordingSink};

    #[tokio::test]
    async fn start_logs_stop_round_trip() {
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<h1>ox</h1>").unwrap();
        let (start, stop, logs) = session_tools(
            DevServerManager::new(),
            "s1",
            dir.path(),
            RecordingSink::new(),
        );

        let out = start
            .invoke(serde_json::json!({
                "command": format!("{py} -m http.server \"$PORT\" --bind 127.0.0.1")
            }))
            .await
            .unwrap();
        assert!(out.contains("running at http://"), "out: {out}");

        let out = logs.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.contains("status: Ready"), "out: {out}");

        let out = stop.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.contains("stopped"), "out: {out}");
        let out = stop.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.contains("No dev server"), "out: {out}");

        // The successful start must be remembered for future sessions.
        let saved = crate::config::load(dir.path());
        assert_eq!(saved.servers.len(), 1);
        assert!(saved.servers[0].command.contains("http.server"));
        assert!(saved.servers[0].auto_port);
    }

    #[tokio::test]
    async fn start_failure_surfaces_output_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let (start, _stop, _logs) = session_tools(
            DevServerManager::new(),
            "s1",
            dir.path(),
            RecordingSink::new(),
        );
        let err = start
            .invoke(serde_json::json!({"command": "echo kaboom && exit 1"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("kaboom"), "err: {err}");
        // A failed start must not be remembered.
        assert!(crate::config::load(dir.path()).servers.is_empty());
    }

    #[tokio::test]
    async fn a_crashed_server_tells_the_model_how_to_recover() {
        // The model's normal way of noticing a crash is a failed tool call:
        // the error must name the cause and the fix, not just "no preview".
        let Some(py) = python3() else { return };
        let dir = tempfile::tempdir().unwrap();
        let manager = DevServerManager::new();
        let server = manager
            .start(
                "s1",
                crate::ServerSpec {
                    name: "dev".into(),
                    command: format!(
                        "{py} -m http.server \"$PORT\" --bind 127.0.0.1 & \
                         pid=$!; sleep 0.5; echo 'boom: config error' >&2; kill $pid; wait"
                    ),
                    port: None,
                    auto_port: true,
                },
                dir.path(),
                RecordingSink::new(),
                std::time::Duration::from_secs(30),
            )
            .await
            .unwrap();

        // Wait for the watchdog to notice the crash.
        for _ in 0..80 {
            if server.status().phase == crate::PreviewPhase::Error {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(server.status().phase, crate::PreviewPhase::Error);

        struct DeadLens;
        #[async_trait]
        impl PreviewLens for DeadLens {
            async fn screenshot(&self) -> Result<Vec<u8>, String> {
                panic!("must not try to screenshot a dead server");
            }
            fn console_tail(&self, _n: usize) -> Vec<crate::console::ConsoleLine> {
                Vec::new()
            }
        }
        let (shot, _console) = sight_tools(manager, "s1", Arc::new(DeadLens));
        let err = shot.invoke(serde_json::json!({})).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not running"), "msg: {msg}");
        assert!(msg.contains("start_dev_server"), "msg: {msg}");
        assert!(msg.contains("boom: config error"), "logs missing: {msg}");
    }

    #[test]
    fn start_schema_stays_lean() {
        // The whole trio is resent on every model call; keep the schemas tiny.
        let schema = harness_tools::schema_for::<StartDevServerArgs>();
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(props.len(), 4);
        assert_eq!(schema["required"].as_array().unwrap().len(), 1);
    }
}
