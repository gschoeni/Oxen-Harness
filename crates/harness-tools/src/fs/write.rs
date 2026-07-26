//! `write_file` — create a file, or replace one wholesale.
//!
//! Creating is unrestricted; replacing an existing file is an edit by another
//! name and answers to the same freshness contract (see [`super::state`]).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

use super::relative_to;
use super::state::FileState;
use super::syntax;
use super::WRITE_FILE_TOOL;

/// Create or overwrite a text file, creating parent directories as needed.
pub struct WriteFileTool {
    workspace: Workspace,
    state: Arc<FileState>,
}

impl WriteFileTool {
    /// A standalone write tool with its own (ungated) session state.
    pub fn new(workspace: Workspace) -> Self {
        Self::with_state(workspace, FileState::ungated())
    }

    /// A write tool sharing session state, so overwriting an existing file is
    /// held to the same freshness contract as editing one.
    pub fn with_state(workspace: Workspace, state: Arc<FileState>) -> Self {
        Self { workspace, state }
    }

    fn relative(&self, resolved: &Path) -> std::path::PathBuf {
        relative_to(&self.workspace, resolved)
    }
}

/// Arguments to `write_file`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct WriteFileArgs {
    /// Path relative to the workspace root; parent directories are created.
    pub path: String,
    /// The full file contents to write.
    pub contents: String,
}

#[async_trait]
impl TypedTool for WriteFileTool {
    const NAME: &'static str = WRITE_FILE_TOOL;
    type Args = WriteFileArgs;

    fn description(&self) -> &str {
        "Create or overwrite a text file at a path relative to the workspace root. \
         Overwriting an existing file requires having read it first (use `edit_file` \
         for changes to part of a file)."
    }

    async fn run(&self, args: WriteFileArgs) -> Result<String, ToolError> {
        let path = self.workspace.resolve(&args.path)?;
        // Held across the check and the write so a concurrent lane can't slip
        // a change in between them.
        let _guard = self.state.lock(&path).await;
        // Creating a new file is unrestricted; replacing one wholesale is an
        // edit by another name and answers to the same contract.
        let before = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ToolError::Execution(format!(
                    "read existing {} before overwrite: {error}",
                    path.display()
                )))
            }
        };
        if let Some(current) = &before {
            self.state.guard(&args.path, &path, current)?;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &args.contents)
            .await
            .map_err(|e| ToolError::Execution(format!("write {}: {e}", path.display())))?;
        // The model has now seen this content — it wrote it — so a follow-up
        // edit in the same turn doesn't have to re-read first.
        self.state.record(&path, &args.contents);
        let mut result = format!("wrote {} bytes to {}", args.contents.len(), path.display());
        if let Some(note) = syntax::regression_note(&path, before.as_deref(), &args.contents) {
            result.push_str(&format!("\n{note}"));
        }
        result.push_str(&self.state.take_rules_for(&self.relative(&path)));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {

    use crate::fs::testkit::*;
    use crate::fs::PathRule;
    use crate::{ToolError, TypedTool};

    #[tokio::test]
    async fn write_then_read_round_trips_raw() {
        let (_dir, ws) = workspace();
        write(&ws, "src/a.txt", "hello").await;
        assert_eq!(read_raw(&ws, "src/a.txt").await, "hello");
    }

    #[tokio::test]
    async fn creating_a_file_is_free_but_overwriting_an_unread_one_is_not() {
        let (_dir, ws) = workspace();
        let (_read, write_tool, _edit, _state) = gated_tools(&ws);

        // A brand new file: nothing to revert, nothing to check.
        write_tool
            .invoke(serde_json::json!({"path": "new.rs", "contents": "fn main() {}"}))
            .await
            .unwrap();

        // An existing file the model hasn't seen: same contract as an edit.
        write(&ws, "old.rs", "important\n").await;
        let err = write_tool
            .invoke(serde_json::json!({"path": "old.rs", "contents": "clobbered"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        assert_eq!(read_raw(&ws, "old.rs").await, "important\n");
    }

    #[tokio::test]
    async fn an_existing_non_utf8_file_is_never_treated_as_new() {
        let (_dir, ws) = workspace();
        let (_read, write_tool, _edit, _state) = gated_tools(&ws);
        let path = ws.resolve("image.bin").unwrap();
        let original = [0_u8, 159, 146, 150, 255];
        tokio::fs::write(&path, original).await.unwrap();

        let error = write_tool
            .invoke(serde_json::json!({"path": "image.bin", "contents": "clobbered"}))
            .await
            .unwrap_err();

        assert!(matches!(error, ToolError::Execution(_)), "{error}");
        assert_eq!(tokio::fs::read(path).await.unwrap(), original);
    }

    #[tokio::test]
    async fn a_path_scoped_convention_rides_along_on_the_first_touch() {
        let (_dir, ws) = workspace();
        write(&ws, "app/main.tsx", "export const App = () => null;\n").await;
        write(&ws, "lib.rs", "fn main() {}\n").await;
        let (read, _write, _edit, state) = gated_tools(&ws);
        state.set_rules(vec![PathRule::new(
            "app/AGENTS.md",
            "Frontend: no default exports.",
            &["app/**".to_string()],
        )]);

        let first = read
            .invoke(serde_json::json!({"path": "app/main.tsx"}))
            .await
            .unwrap();
        assert!(first.contains("no default exports"), "got: {first}");

        // Once only, and never on a path the rule doesn't govern.
        let again = read
            .invoke(serde_json::json!({"path": "app/main.tsx"}))
            .await
            .unwrap();
        assert!(!again.contains("no default exports"));
        let elsewhere = read
            .invoke(serde_json::json!({"path": "lib.rs"}))
            .await
            .unwrap();
        assert!(!elsewhere.contains("no default exports"));
    }
}
