//! Git tool: status, diff, log, and commit, scoped to the workspace root.
//!
//! This shells out to the system `git` binary (kept simple and robust rather
//! than linking libgit2). Only an allow-list of operations is exposed.

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

/// Tool name for [`GitTool`].
pub const GIT_TOOL: &str = "git";

/// Perform a git operation in the workspace.
pub struct GitTool {
    workspace: Workspace,
}

impl GitTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }

    async fn run_git(&self, args: &[String]) -> Result<String, ToolError> {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(self.workspace.root())
            .output()
            .await
            .map_err(|e| ToolError::Execution(format!("spawn git: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            Ok(stdout.into_owned())
        } else {
            Err(ToolError::Execution(format!(
                "git {} failed: {}",
                args.join(" "),
                stderr.trim()
            )))
        }
    }
}

/// The allow-listed git operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitOperation {
    /// Working-tree status (`git status --short`).
    Status,
    /// Unstaged changes (`git diff`).
    Diff,
    /// Recent commits (`git log --oneline`).
    Log,
    /// Stage everything and commit (requires `message`).
    Commit,
}

/// Arguments to `git`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct GitArgs {
    /// The operation to perform.
    pub operation: GitOperation,
    /// Commit message (required when operation is `commit`).
    pub message: Option<String>,
    /// For `log`: number of commits to show (default 20).
    pub max_count: Option<u64>,
}

#[async_trait]
impl TypedTool for GitTool {
    const NAME: &'static str = GIT_TOOL;
    type Args = GitArgs;

    fn description(&self) -> &str {
        "Run a git operation in the workspace: `status`, `diff`, `log`, or `commit`. \
         `commit` stages all changes (git add -A) and commits with `message`."
    }

    async fn run(&self, args: GitArgs) -> Result<String, ToolError> {
        match args.operation {
            GitOperation::Status => self.run_git(&["status".into(), "--short".into()]).await,
            GitOperation::Diff => self.run_git(&["diff".into()]).await,
            GitOperation::Log => {
                let n = args.max_count.unwrap_or(20);
                self.run_git(&["log".into(), "--oneline".into(), "-n".into(), n.to_string()])
                    .await
            }
            GitOperation::Commit => {
                let message = args
                    .message
                    .as_deref()
                    .filter(|m| !m.trim().is_empty())
                    .ok_or_else(|| {
                        ToolError::InvalidArguments("`commit` requires `message`".into())
                    })?;
                let add = self.run_git(&["add".into(), "-A".into()]).await?;
                let commit = self
                    .run_git(&["commit".into(), "-m".into(), message.to_string()])
                    .await?;
                Ok(format!("{add}{commit}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git_workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let git = GitTool::new(ws.clone());
        git.run_git(&["init".into(), "-q".into()]).await.unwrap();
        // Local identity so commits work without global git config.
        git.run_git(&["config".into(), "user.email".into(), "ox@oxen.ai".into()])
            .await
            .unwrap();
        git.run_git(&["config".into(), "user.name".into(), "Ox".into()])
            .await
            .unwrap();
        (dir, ws)
    }

    #[tokio::test]
    async fn status_shows_untracked_file() {
        let (dir, ws) = git_workspace().await;
        std::fs::write(dir.path().join("new.txt"), "hi").unwrap();
        let out = GitTool::new(ws)
            .invoke(serde_json::json!({"operation": "status"}))
            .await
            .unwrap();
        assert!(out.contains("new.txt"));
    }

    #[tokio::test]
    async fn commit_then_log_shows_message() {
        let (dir, ws) = git_workspace().await;
        std::fs::write(dir.path().join("a.txt"), "content").unwrap();
        let git = GitTool::new(ws);
        git.invoke(serde_json::json!({"operation": "commit", "message": "first ox commit"}))
            .await
            .unwrap();
        let log = git
            .invoke(serde_json::json!({"operation": "log"}))
            .await
            .unwrap();
        assert!(log.contains("first ox commit"));
    }

    #[tokio::test]
    async fn commit_without_message_errors() {
        let (_dir, ws) = git_workspace().await;
        let err = GitTool::new(ws)
            .invoke(serde_json::json!({"operation": "commit"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
