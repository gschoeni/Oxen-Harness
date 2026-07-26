//! Shell execution tool.
//!
//! Commands run inside the workspace (the sandbox), capturing stdout, stderr,
//! and the exit code. The model decides what to run; confining the cwd keeps
//! execution scoped to the open project. A timeout guards against hung
//! commands and output is capped so a runaway command cannot blow up the
//! model's context.
//!
//! Successive commands share a working directory and environment — see
//! [`session`] for why that isn't a long-lived shell process.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

pub mod session;

use session::ShellSession;

/// Tool name for [`ShellTool`].
pub const RUN_SHELL_TOOL: &str = "run_shell";

/// Default command timeout (2 minutes), matching common agent shells.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// Hard cap on how much stdout/stderr (each) is returned to the model.
const MAX_STREAM_CHARS: usize = 30_000;

/// Run a shell command inside the workspace root.
pub struct ShellTool {
    workspace: Workspace,
    /// Background-task registry (shared with `task_output`/`kill_task`).
    /// `Some` enables `is_background` and timeout auto-backgrounding; `None`
    /// keeps the legacy behavior (a timed-out command is killed).
    tasks: Option<std::sync::Arc<crate::tasks::BackgroundTasks>>,
    /// Directory and environment carried between calls. Shared by `Arc` for
    /// the same reason the fs tools share their state: fleet lanes run against
    /// one registry, so they see one shell.
    session: Arc<Mutex<ShellSession>>,
}

impl ShellTool {
    pub fn new(workspace: Workspace) -> Self {
        let session = Arc::new(Mutex::new(ShellSession::new(workspace.root())));
        Self {
            workspace,
            tasks: None,
            session,
        }
    }

    /// A shell tool with background-task support: `is_background: true` runs
    /// detached, and a foreground command that hits its timeout converts to a
    /// background task instead of being killed. Share the same registry with
    /// the [`TaskOutputTool`]/[`KillTaskTool`] pair so ids resolve.
    ///
    /// [`TaskOutputTool`]: crate::tasks::TaskOutputTool
    /// [`KillTaskTool`]: crate::tasks::KillTaskTool
    pub fn with_tasks(
        workspace: Workspace,
        tasks: std::sync::Arc<crate::tasks::BackgroundTasks>,
    ) -> Self {
        let session = Arc::new(Mutex::new(ShellSession::new(workspace.root())));
        Self {
            workspace,
            tasks: Some(tasks),
            session,
        }
    }

    /// The directory the next command will run in.
    fn cwd(&self) -> std::path::PathBuf {
        self.session
            .lock()
            .expect("shell session")
            .cwd()
            .to_path_buf()
    }

    /// Environment overrides for the next command.
    fn env(&self) -> std::collections::BTreeMap<String, String> {
        self.session.lock().expect("shell session").env().clone()
    }

    /// Fold a finished command's directory/environment back into the session.
    /// A missing or unreadable report (the command called `exit`, or the
    /// shell died) simply leaves the previous state in place.
    fn absorb(&self, carrier: Option<&StateFile>) -> Option<String> {
        let report = std::fs::read_to_string(carrier?.path()).ok()?;
        self.session
            .lock()
            .expect("shell session")
            .absorb(&report, self.workspace.root())
    }
}

/// Arguments to `run_shell`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct ShellArgs {
    /// Command line to execute via the shell.
    pub command: String,
    /// Timeout in milliseconds (default 120000).
    pub timeout_ms: Option<u64>,
    /// Run detached: returns a task id immediately instead of waiting. Use
    /// for servers and long builds; check on it with `task_output`.
    pub is_background: Option<bool>,
}

#[async_trait]
impl TypedTool for ShellTool {
    const NAME: &'static str = RUN_SHELL_TOOL;
    type Args = ShellArgs;

    fn description(&self) -> &str {
        "Run a shell command. Returns exit code, stdout, and stderr. The working directory \
         and exported variables persist between calls (starting at the project root), so \
         `cd` and `export` stick and need not be repeated. Times out after 2 minutes \
         (`timeout_ms`); a timed-out command continues as a background task \
         (`task_output`). Start servers and long builds with `is_background: true`. Prefer \
         the dedicated tools for file work: `find_files`/`search_files`/`read_file` over \
         `find`/`grep`/`cat`, and `write_file`/`edit_file` over redirects/`sed`."
    }

    async fn run(&self, args: ShellArgs) -> Result<String, ToolError> {
        let command = &args.command;
        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let background = args.is_background.unwrap_or(false);
        let (cwd, env) = (self.cwd(), self.env());
        // A background command inherits the session's directory and
        // environment but never changes them: a dev server left running must
        // not decide where the next foreground command lands.
        let carrier = (!background).then(StateFile::new);
        let to_run = match &carrier {
            Some(state) => ShellSession::wrap(command, state.path()),
            None => command.clone(),
        };

        if let Some(tasks) = &self.tasks {
            let id = tasks.spawn(&to_run, &cwd, MAX_STREAM_CHARS, &env).await?;
            if background {
                return Ok(format!(
                    "started background task {id}: {command}\n\
                     Check on it with task_output (task_id: {id}); stop it with kill_task. \
                     Do not poll in a sleep loop — do other useful work between checks."
                ));
            }
            return match tasks.wait(id, Duration::from_millis(timeout_ms)).await {
                Some(_) => {
                    let (exit, stdout, stderr, overflow) = tasks
                        .take_streams(id)
                        .await
                        .ok_or_else(|| ToolError::Execution("task vanished".into()))?;
                    let mut out = format_streams(exit.code, &stdout, &stderr);
                    // Output past the cap isn't gone, just parked.
                    if let Some(marker) = overflow {
                        out.push_str(&format!(
                            "\n[the omitted middle is kept: call retrieve_original with {marker}]"
                        ));
                    }
                    if let Some(note) = self.absorb(carrier.as_ref()) {
                        out.push_str(&format!("\n[{note}]"));
                    }
                    Ok(out)
                }
                // The timeout is a patience limit, not a kill switch: the
                // command keeps running as a background task, so slow builds
                // and accidentally-foregrounded servers are never lost work.
                // Bounded, though — past the cap of live tasks, revert to the
                // classic kill so runaway commands can't accumulate forever.
                None => {
                    // The command's trailer may create this file only when the
                    // now-background task eventually exits. Keep its RAII owner
                    // alive until then so the final environment (including
                    // credentials) cannot be stranded in the temp directory.
                    if let (Some(carrier), Some(done)) = (carrier, tasks.completion(id).await) {
                        carrier.remove_when_done(done);
                    }
                    if tasks.running_count().await > crate::tasks::MAX_AUTO_BACKGROUND_TASKS {
                        let _ = tasks.kill(id).await;
                        return Ok(format!(
                            "exit_code: timeout\ncommand exceeded {timeout_ms} ms and was \
                             terminated ({} background tasks are already running — check or \
                             kill some with task_output/kill_task): {command}",
                            crate::tasks::MAX_AUTO_BACKGROUND_TASKS
                        ));
                    }
                    // Show what it was doing, so the model can judge whether
                    // to keep it or kill_task it.
                    let tail = tasks.peek_tail(id, 2_000).await;
                    Ok(format!(
                        "exit_code: still-running\ncommand exceeded {timeout_ms} ms and now \
                         continues as background task {id}: {command}\n\
                         Check on it with task_output (task_id: {id}); stop it with kill_task.\n\
                         --- output so far ---\n{tail}"
                    ))
                }
            };
        }

        // Legacy path (no task registry): bounded capture, kill on timeout.
        let output = crate::process::run_bounded(
            shell_command(&to_run).current_dir(&cwd).envs(&env),
            Duration::from_millis(timeout_ms),
            MAX_STREAM_CHARS,
        )
        .await
        .map_err(|e| ToolError::Execution(format!("spawn `{command}`: {e}")))?;
        if output.timed_out {
            return Ok(format!(
                "exit_code: timeout\ncommand exceeded {timeout_ms} ms and was terminated: {command}"
            ));
        }
        let mut out = format_streams(output.code, &output.stdout, &output.stderr);
        if let Some(note) = self.absorb(carrier.as_ref()) {
            out.push_str(&format!("\n[{note}]"));
        }
        Ok(out)
    }
}

/// The temp file one command writes its final directory and environment to,
/// removed when the call is done. A file rather than an extra file descriptor
/// because `tokio::process` offers no portable way to hand a child fd 3, and
/// the state is small and short-lived.
struct StateFile(std::path::PathBuf);

impl StateFile {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "oxen-harness-shell-{}-{n}.state",
            std::process::id()
        )))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn remove_when_done(
        self,
        mut done: tokio::sync::watch::Receiver<Option<crate::tasks::TaskExit>>,
    ) {
        tokio::spawn(async move {
            loop {
                if done.borrow().is_some() || done.changed().await.is_err() {
                    break;
                }
            }
            drop(self);
        });
    }
}

impl Drop for StateFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The classic `run_shell` result shape: exit code, stdout, stderr.
fn format_streams(code: Option<i32>, stdout: &str, stderr: &str) -> String {
    let code = code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string());
    format!("exit_code: {code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}")
}

#[cfg(windows)]
pub(crate) fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
pub(crate) fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_command_and_captures_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let out = ShellTool::new(ws)
            .invoke(serde_json::json!({"command": "echo hello-ox"}))
            .await
            .unwrap();
        assert!(out.contains("hello-ox"));
        assert!(out.contains("exit_code: 0"));
    }

    #[tokio::test]
    async fn runs_in_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let out = ShellTool::new(ws)
            .invoke(serde_json::json!({"command": "ls"}))
            .await
            .unwrap();
        assert!(out.contains("marker.txt"));
    }

    #[tokio::test]
    async fn times_out_long_commands() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let out = ShellTool::new(ws)
            .invoke(serde_json::json!({"command": "sleep 5", "timeout_ms": 100}))
            .await
            .unwrap();
        assert!(out.contains("exit_code: timeout"));
    }

    #[tokio::test]
    async fn reports_nonzero_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let out = ShellTool::new(ws)
            .invoke(serde_json::json!({"command": "exit 3"}))
            .await
            .unwrap();
        assert!(out.contains("exit_code: 3"));
    }

    fn task_shell(
        dir: &std::path::Path,
    ) -> (ShellTool, std::sync::Arc<crate::tasks::BackgroundTasks>) {
        let ws = Workspace::new(dir).unwrap();
        let tasks = crate::tasks::BackgroundTasks::new(dir.join(".task-logs"));
        (ShellTool::with_tasks(ws, tasks.clone()), tasks)
    }

    #[tokio::test]
    async fn background_command_returns_a_task_id_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, tasks) = task_shell(dir.path());
        let out = tool
            .invoke(serde_json::json!({"command": "echo bg-hi", "is_background": true}))
            .await
            .unwrap();
        assert!(out.contains("started background task 1"), "{out}");
        // The task ran for real: wait, then read its output through the registry.
        tasks
            .wait(1, std::time::Duration::from_secs(10))
            .await
            .expect("task should finish");
        let report = tasks.output(1).await.unwrap();
        assert!(report.contains("bg-hi"), "{report}");
    }

    #[tokio::test]
    async fn foreground_timeout_converts_to_a_background_task() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, tasks) = task_shell(dir.path());
        let out = tool
            .invoke(serde_json::json!({"command": "sleep 5; echo finally", "timeout_ms": 100}))
            .await
            .unwrap();
        // Not killed: converted, with the id to follow up on.
        assert!(out.contains("continues as background task 1"), "{out}");
        let report = tasks.output(1).await.unwrap();
        assert!(report.contains("running"), "{report}");
        // Clean up so the sleep doesn't outlive the test.
        tasks.kill(1).await.unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_completed_timed_out_command_removes_its_state_carrier() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, tasks) = task_shell(dir.path());
        let secret = format!(
            "carrier-secret-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let command = format!("sleep 0.1; export OXEN_CARRIER_SECRET={secret}");
        let out = tool
            .invoke(serde_json::json!({"command": command, "timeout_ms": 10}))
            .await
            .unwrap();
        assert!(out.contains("continues as background task 1"), "{out}");
        tasks
            .wait(1, Duration::from_secs(10))
            .await
            .expect("task should finish");

        // The cleanup listener and the task completion signal are scheduled
        // independently; yield briefly until the listener observes completion.
        for _ in 0..20 {
            tokio::task::yield_now().await;
            let leaked = std::fs::read_dir(std::env::temp_dir())
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("oxen-harness-shell-")
                })
                .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
                .any(|contents| contents.contains(&secret));
            if !leaked {
                return;
            }
        }
        panic!("the completed command left its environment carrier behind");
    }

    #[tokio::test]
    async fn foreground_with_tasks_keeps_the_classic_output_shape() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, _tasks) = task_shell(dir.path());
        let out = tool
            .invoke(serde_json::json!({"command": "echo classic; echo err >&2; exit 3"}))
            .await
            .unwrap();
        assert!(out.contains("exit_code: 3"), "{out}");
        assert!(out.contains("--- stdout ---"), "{out}");
        assert!(out.contains("classic"), "{out}");
        assert!(out.contains("--- stderr ---"), "{out}");
        assert!(out.contains("err"), "{out}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_cd_persists_to_the_next_command() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/marker.txt"), "x").unwrap();
        let tool = ShellTool::new(Workspace::new(dir.path()).unwrap());

        let first = tool
            .invoke(serde_json::json!({"command": "cd src"}))
            .await
            .unwrap();
        assert!(first.contains("working directory is now"), "{first}");

        // Without persistence this would list the project root instead.
        let second = tool
            .invoke(serde_json::json!({"command": "ls"}))
            .await
            .unwrap();
        assert!(second.contains("marker.txt"), "{second}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn an_export_persists_to_the_next_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool::new(Workspace::new(dir.path()).unwrap());

        tool.invoke(serde_json::json!({"command": "export OXEN_TEST_TOKEN=sekret"}))
            .await
            .unwrap();
        let out = tool
            .invoke(serde_json::json!({"command": "echo \"[$OXEN_TEST_TOKEN]\""}))
            .await
            .unwrap();

        assert!(out.contains("[sekret]"), "{out}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_cd_out_of_the_project_does_not_stick() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("root-marker.txt"), "x").unwrap();
        let tool = ShellTool::new(Workspace::new(dir.path()).unwrap());

        let out = tool
            .invoke(serde_json::json!({"command": "cd /"}))
            .await
            .unwrap();
        assert!(out.contains("outside the project"), "{out}");

        let after = tool
            .invoke(serde_json::json!({"command": "ls"}))
            .await
            .unwrap();
        assert!(after.contains("root-marker.txt"), "{after}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn the_wrapper_does_not_disturb_exit_codes_or_output() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool::new(Workspace::new(dir.path()).unwrap());

        let out = tool
            .invoke(serde_json::json!({"command": "echo out; echo err >&2; exit 7"}))
            .await
            .unwrap();

        assert!(out.contains("exit_code: 7"), "{out}");
        assert!(out.contains("out"), "{out}");
        assert!(out.contains("err"), "{out}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_background_command_inherits_the_directory_without_changing_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let (tool, tasks) = task_shell(dir.path());
        tool.invoke(serde_json::json!({"command": "cd src"}))
            .await
            .unwrap();

        let out = tool
            .invoke(serde_json::json!({"command": "pwd", "is_background": true}))
            .await
            .unwrap();
        assert!(out.contains("started background task"), "{out}");
        tasks
            .wait(2, std::time::Duration::from_secs(10))
            .await
            .expect("task should finish");
        let report = tasks.output(2).await.unwrap();
        // It ran in the session's directory…
        assert!(report.contains("src"), "{report}");

        // …and left it alone for the next foreground command.
        let after = tool
            .invoke(serde_json::json!({"command": "pwd"}))
            .await
            .unwrap();
        assert!(after.contains("src"), "{after}");
    }

    #[tokio::test]
    async fn truncated_output_is_recoverable_instead_of_lost() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let store = std::sync::Arc::new(harness_compress::CcrStore::default());
        let tasks = crate::tasks::BackgroundTasks::with_overflow(
            dir.path().join(".task-logs"),
            store.clone(),
        );
        let tool = ShellTool::with_tasks(ws, tasks);

        #[cfg(not(windows))]
        let command = "for i in $(seq 1 4000); do echo \"line $i of the build log\"; done";
        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"1..4000 | % { 'line ' + $_ }\"";
        let out = tool
            .invoke(serde_json::json!({"command": command, "timeout_ms": 60000}))
            .await
            .unwrap();

        // The model sees head and tail, and is told the middle is retrievable.
        assert!(out.contains("characters omitted"), "{out}");
        assert!(out.contains("retrieve_original"), "{out}");
        let hash = out
            .split("<<ccr:")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("marker in output")
            .to_string();
        let full = store.get(&hash).expect("full output kept");
        // The middle — the part the model could never otherwise see again.
        assert!(full.contains("line 2000 of the build log"), "middle lost");
    }

    #[tokio::test]
    async fn drains_large_output_but_retains_only_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        #[cfg(not(windows))]
        let command = "yes x | head -c 200000";
        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"'x' * 200000\"";
        let out = ShellTool::new(ws)
            .invoke(serde_json::json!({"command": command}))
            .await
            .unwrap();
        assert!(out.chars().count() < MAX_STREAM_CHARS + 500);
        assert!(out.contains("characters omitted"));
    }
}
