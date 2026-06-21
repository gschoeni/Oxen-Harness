//! Shell execution tool.
//!
//! Commands run with their working directory pinned to the workspace root (the
//! sandbox), capturing stdout, stderr, and the exit code. The model decides
//! what to run; confining the cwd keeps execution scoped to the open project.

use async_trait::async_trait;

use crate::sandbox::Workspace;
use crate::{Tool, ToolError};

/// Run a shell command inside the workspace root.
pub struct ShellTool {
    workspace: Workspace,
}

impl ShellTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "run_shell"
    }
    fn description(&self) -> &str {
        "Run a shell command from the workspace root. Returns exit code, stdout, and stderr."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Command line to execute via the shell." }
            },
            "required": ["command"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing string `command`".into()))?;

        let mut cmd = shell_command(command);
        cmd.current_dir(self.workspace.root());

        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::Execution(format!("spawn `{command}`: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
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
