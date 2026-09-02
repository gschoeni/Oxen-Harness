//! The session's custom slash commands (Markdown prompt templates, see
//! `harness_runtime::commands`), installed once at startup and consulted by
//! the parser and the completion list.
//!
//! A process-wide slot rather than a field threaded through the REPL, the
//! live composer, and the parser: the CLI serves one workspace per process,
//! and `parse_command` is a pure function called from a dozen places.

use std::sync::RwLock;

use harness_runtime::commands::CustomCommand;

static COMMANDS: RwLock<Vec<CustomCommand>> = RwLock::new(Vec::new());
/// The workspace the session serves, recorded alongside its commands so
/// path completion and the git meter read the right tree.
static WORKSPACE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The session's workspace root (falls back to the process directory before
/// `install_for` ran).
pub(crate) fn workspace_root() -> std::path::PathBuf {
    WORKSPACE
        .get()
        .cloned()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// Replace the installed set (discovery runs once per workspace).
pub(crate) fn install(commands: Vec<CustomCommand>) {
    *COMMANDS.write().expect("custom commands lock") = commands;
}

/// Discover the commands visible from `workspace_root` and install them,
/// returning how many were found.
pub(crate) fn install_for(workspace_root: &std::path::Path) -> usize {
    let _ = WORKSPACE.set(workspace_root.to_path_buf());
    let found = harness_runtime::commands::discover(workspace_root);
    let count = found.len();
    install(found);
    count
}

/// The prompt a `/<name> <args>` line expands to, when `name` is a custom
/// command.
pub(crate) fn expand(name: &str, args: &str) -> Option<String> {
    COMMANDS
        .read()
        .ok()?
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.expand(args))
}

/// `(/<name>, description)` for every installed command, for completion.
pub(crate) fn entries() -> Vec<(String, String)> {
    COMMANDS
        .read()
        .map(|cmds| {
            cmds.iter()
                .map(|c| (format!("/{}", c.name), c.description.clone()))
                .collect()
        })
        .unwrap_or_default()
}
