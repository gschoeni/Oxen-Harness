//! Shell execution tool.
//!
//! Commands run with their working directory pinned to the workspace root (the
//! sandbox), capturing stdout, stderr, and the exit code. The model decides
//! what to run; confining the cwd keeps execution scoped to the open project.
//! A timeout guards against hung commands and output is capped so a runaway
//! command cannot blow up the model's context.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

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
}

impl ShellTool {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            tasks: None,
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
        Self {
            workspace,
            tasks: Some(tasks),
        }
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
        "Run a shell command from the workspace root. Returns exit code, stdout, and stderr. \
         Times out after 2 minutes by default (override with `timeout_ms`); a timed-out \
         command keeps running as a background task (check `task_output`). Start servers and \
         long builds with `is_background: true`. Prefer the dedicated tools for file work: \
         `find_files`/`search_files`/`read_file` instead of `find`/`grep`/`cat`, and \
         `write_file`/`edit_file` instead of redirects/`sed`."
    }

    async fn run(&self, args: ShellArgs) -> Result<String, ToolError> {
        let command = &args.command;
        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);

        if let Some(tasks) = &self.tasks {
            let id = tasks
                .spawn(command, self.workspace.root(), MAX_STREAM_CHARS)
                .await?;
            if args.is_background.unwrap_or(false) {
                return Ok(format!(
                    "started background task {id}: {command}\n\
                     Check on it with task_output (task_id: {id}); stop it with kill_task. \
                     Do not poll in a sleep loop — do other useful work between checks."
                ));
            }
            return match tasks.wait(id, Duration::from_millis(timeout_ms)).await {
                Some(_) => {
                    let (exit, stdout, stderr) = tasks
                        .take_streams(id)
                        .await
                        .ok_or_else(|| ToolError::Execution("task vanished".into()))?;
                    Ok(format_streams(exit.code, &stdout, &stderr))
                }
                // The timeout is a patience limit, not a kill switch: the
                // command keeps running as a background task, so slow builds
                // and accidentally-foregrounded servers are never lost work.
                // Bounded, though — past the cap of live tasks, revert to the
                // classic kill so runaway commands can't accumulate forever.
                None => {
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
            shell_command(command).current_dir(self.workspace.root()),
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
        Ok(format_streams(output.code, &output.stdout, &output.stderr))
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

    fn task_shell(dir: &std::path::Path) -> (ShellTool, std::sync::Arc<crate::tasks::BackgroundTasks>) {
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
