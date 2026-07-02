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
}

impl ShellTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

/// Arguments to `run_shell`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct ShellArgs {
    /// Command line to execute via the shell.
    pub command: String,
    /// Timeout in milliseconds (default 120000).
    pub timeout_ms: Option<u64>,
}

#[async_trait]
impl TypedTool for ShellTool {
    const NAME: &'static str = RUN_SHELL_TOOL;
    type Args = ShellArgs;

    fn description(&self) -> &str {
        "Run a shell command from the workspace root. Returns exit code, stdout, and stderr. \
         Times out after 2 minutes by default (override with `timeout_ms`). Prefer the \
         dedicated tools for file work: use `find_files`/`search_files`/`read_file` instead \
         of `find`/`grep`/`cat`, and `write_file`/`edit_file` instead of redirects/`sed`."
    }

    async fn run(&self, args: ShellArgs) -> Result<String, ToolError> {
        let command = &args.command;
        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);

        let mut cmd = shell_command(command);
        cmd.current_dir(self.workspace.root());

        let output = match tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output())
            .await
        {
            Ok(result) => {
                result.map_err(|e| ToolError::Execution(format!("spawn `{command}`: {e}")))?
            }
            Err(_) => {
                return Ok(format!(
                    "exit_code: timeout\ncommand exceeded {timeout_ms} ms and was abandoned: {command}"
                ));
            }
        };

        let stdout = cap(&String::from_utf8_lossy(&output.stdout));
        let stderr = cap(&String::from_utf8_lossy(&output.stderr));
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());

        Ok(format!(
            "exit_code: {code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        ))
    }
}

/// Truncate a stream to [`MAX_STREAM_CHARS`], noting how much was dropped.
fn cap(s: &str) -> String {
    if s.chars().count() <= MAX_STREAM_CHARS {
        return s.to_string();
    }
    let kept: String = s.chars().take(MAX_STREAM_CHARS).collect();
    format!("{kept}\n… [output truncated at {MAX_STREAM_CHARS} chars]")
}

#[cfg(windows)]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> tokio::process::Command {
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
}
