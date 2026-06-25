//! Workspace scoping: keeps tool file access pointed at one working directory.
//!
//! Sessions are scoped to one working directory. Every path a tool touches is
//! resolved through [`Workspace::resolve`], which rejects paths that escape the
//! root (e.g. `../../etc/passwd`). The model still decides *what* to do; this
//! keeps it pointed at the project the user opened.
//!
//! **This is a policy, not a security boundary.** Treat it as a guardrail
//! against an honest agent wandering, not a sandbox against a malicious one:
//!
//! - [`Workspace::resolve`] is **lexical** — it normalizes `.`/`..` textually and
//!   does not call `canonicalize`, so a **symlink inside the root that points
//!   outside it is not detected**: reads/writes through that link land wherever
//!   it targets. (The root itself is canonicalized in [`Workspace::new`].)
//! - `run_shell` is only cwd-pinned to the root; a shell command can still `cd`
//!   elsewhere, read absolute paths, or reach the network. It is not confined.
//!
//! Real isolation (containers, seccomp, a permission/approval layer) is a
//! separate concern layered above this; see `04-backlog.md`.

use std::path::{Component, Path, PathBuf};

use crate::ToolError;

/// A working directory that tool file access is confined to.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Create a workspace rooted at `root`, canonicalizing it so symlinks and
    /// `.`/`..` segments in the root itself are resolved up front.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let root = root.as_ref();
        let canonical = root
            .canonicalize()
            .map_err(|e| ToolError::Execution(format!("workspace root {}: {e}", root.display())))?;
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a (possibly relative) path against the workspace root, rejecting
    /// anything that would escape it.
    ///
    /// This is lexical: it does not require the path to exist, so it works for
    /// files the agent is about to create.
    pub fn resolve(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        let path = path.as_ref();
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };

        let mut normalized = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(escape_error(path));
                    }
                }
                Component::CurDir => {}
                other => normalized.push(other.as_os_str()),
            }
        }

        if normalized.starts_with(&self.root) {
            Ok(normalized)
        } else {
            Err(escape_error(path))
        }
    }
}

fn escape_error(path: &Path) -> ToolError {
    ToolError::InvalidArguments(format!(
        "path {} escapes the workspace sandbox",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        (dir, ws)
    }

    #[test]
    fn resolves_relative_paths_inside_root() {
        let (_dir, ws) = workspace();
        let resolved = ws.resolve("src/main.rs").unwrap();
        assert!(resolved.starts_with(ws.root()));
        assert!(resolved.ends_with("src/main.rs"));
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let (_dir, ws) = workspace();
        let err = ws.resolve("../secrets.txt").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn rejects_absolute_path_outside_root() {
        let (_dir, ws) = workspace();
        let err = ws.resolve("/etc/passwd").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn allows_nested_dotdot_that_stays_inside() {
        let (_dir, ws) = workspace();
        let resolved = ws.resolve("a/b/../c.txt").unwrap();
        assert!(resolved.ends_with("a/c.txt"));
    }
}
