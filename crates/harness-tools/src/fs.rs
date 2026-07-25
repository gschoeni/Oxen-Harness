//! Filesystem tools: read, write, edit, find (glob), and search (grep).
//!
//! These mirror the essential file primitives a strong coding agent expects
//! (read with line numbers + offset/limit, exact-string edit, glob file
//! discovery, and ripgrep-style regex search), all confined to the
//! [`Workspace`] sandbox.

use std::path::Path;

use crate::sandbox::Workspace;

mod edit;
mod edit_diagnostics;
mod find;
pub mod outline;
mod read;
pub mod state;
pub mod syntax;
mod write;

pub use edit::{EditFileArgs, EditFileTool, Replacement};
pub use find::{FindFilesArgs, FindFilesTool, OutputMode, SearchArgs, SearchTool};
pub use read::{ReadFileArgs, ReadFileTool};
pub use state::{FileState, Freshness, PathRule};
pub use write::{WriteFileArgs, WriteFileTool};

/// Tool name for [`ReadFileTool`].
pub const READ_FILE_TOOL: &str = "read_file";
/// Tool name for [`WriteFileTool`].
pub const WRITE_FILE_TOOL: &str = "write_file";
/// Tool name for [`EditFileTool`].
pub const EDIT_FILE_TOOL: &str = "edit_file";
/// Tool name for [`FindFilesTool`].
pub const FIND_FILES_TOOL: &str = "find_files";
/// Tool name for [`SearchTool`].
pub const SEARCH_FILES_TOOL: &str = "search_files";

/// `read_file` reads at most this many lines when no `limit` is given.
const DEFAULT_READ_LIMIT: usize = 2000;
/// Lines longer than this are truncated in `read_file` output.
const MAX_LINE_LEN: usize = 2000;
/// Total text returned by one read, independent of the requested line count.
const MAX_READ_CHARS: usize = 100_000;
/// Files larger than this are skipped by the in-process regex search.
const MAX_SEARCH_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Default cap on `find_files` / `search_files` results.
const DEFAULT_MAX_RESULTS: usize = 200;

/// A resolved path expressed relative to the workspace root.
///
/// Path-scoped conventions are written as globs against the project (`app/**`),
/// so they must be matched against that shape — not the model's raw argument,
/// which `Workspace::resolve` also accepts as absolute or `./`-prefixed. A
/// model that pasted an absolute path out of a grep would otherwise never see
/// the convention governing it.
fn relative_to(workspace: &Workspace, resolved: &Path) -> std::path::PathBuf {
    resolved
        .strip_prefix(workspace.root())
        .unwrap_or(resolved)
        .to_path_buf()
}

/// Scaffolding shared by the fs tools' tests: a sandboxed workspace, raw-file
/// helpers, and the gated read/write/edit trio the shipped registry builds.
#[cfg(test)]
pub(crate) mod testkit {
    use std::sync::Arc;

    use super::*;
    use crate::TypedTool;

    pub(crate) fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        (dir, ws)
    }

    pub(crate) async fn write(ws: &Workspace, path: &str, contents: &str) {
        WriteFileTool::new(ws.clone())
            .invoke(serde_json::json!({ "path": path, "contents": contents }))
            .await
            .unwrap();
    }

    pub(crate) async fn read_raw(ws: &Workspace, path: &str) -> String {
        tokio::fs::read_to_string(ws.resolve(path).unwrap())
            .await
            .unwrap()
    }

    /// The shipped registry's shape: read/write/edit behind one gated state.
    pub(crate) fn gated_tools(
        ws: &Workspace,
    ) -> (ReadFileTool, WriteFileTool, EditFileTool, Arc<FileState>) {
        let state = FileState::gated();
        (
            ReadFileTool::with_state(ws.clone(), state.clone()),
            WriteFileTool::with_state(ws.clone(), state.clone()),
            EditFileTool::with_state(ws.clone(), state.clone()),
            state,
        )
    }

    /// A Rust file long enough to outline: a visible signature, a long body.
    pub(crate) fn long_source() -> String {
        let mut src = String::from("pub struct Config {\n    pub name: String,\n}\n\nimpl Config {\n    pub fn load() -> Self {\n");
        for i in 0..250 {
            src.push_str(&format!("        let step_{i} = {i};\n"));
        }
        src.push_str("        Self { name: String::new() }\n    }\n}\n");
        src
    }
}
