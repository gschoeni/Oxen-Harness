//! Pre-command workspace snapshots.
//!
//! Before an approved *dangerous* command runs, the workspace is committed
//! into a shadow git repository under `~/.oxen-harness/snapshots/` (the
//! workspace's own `.git`, if any, is untouched — the shadow repo just points
//! its work-tree at the workspace). A wrong approval is then recoverable with
//! plain git tooling:
//!
//! ```text
//! git --git-dir ~/.oxen-harness/snapshots/<name> --work-tree <workspace> checkout <hash> -- .
//! ```
//!
//! Snapshots are best-effort by design: a failure is logged to the audit
//! trail, never a reason to block a command the user just approved. They also
//! respect the workspace's `.gitignore`, so `node_modules`/`target` don't
//! balloon the shadow repo.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Take a snapshot of `workspace`, returning the shadow commit hash.
pub(crate) fn take(workspace: &Path) -> Result<String, String> {
    let shadow = shadow_dir(workspace)?;
    if !shadow.join("HEAD").exists() {
        std::fs::create_dir_all(&shadow).map_err(|e| format!("create shadow dir: {e}"))?;
        run(workspace, &shadow, &["init", "-q"])?;
    }
    run(workspace, &shadow, &["add", "-A"])?;
    // An empty commit result (nothing changed since the last snapshot) is fine:
    // the previous snapshot already covers this state.
    let _ = run(
        workspace,
        &shadow,
        &[
            "-c",
            "user.name=oxen-harness",
            "-c",
            "user.email=harness@localhost",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "snapshot before approved dangerous command",
        ],
    );
    run(workspace, &shadow, &["rev-parse", "--short", "HEAD"])
}

/// One shadow repo per workspace, named after its sanitized absolute path
/// (the same convention the project store uses for per-project state).
fn shadow_dir(workspace: &Path) -> Result<PathBuf, String> {
    let root = harness_config::paths::snapshots_dir()
        .map_err(|e| format!("resolve snapshots dir: {e}"))?;
    let name: String = workspace
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Ok(root.join(name))
}

fn run(workspace: &Path, shadow: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(shadow)
        .arg("--work-tree")
        .arg(workspace)
        .args(args)
        // Don't let an ambient git environment redirect the shadow repo.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .current_dir(workspace)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_commits_workspace_state_into_a_shadow_repo() {
        let _env = crate::testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("keep.txt"), "precious").unwrap();

        let hash = take(ws.path()).expect("snapshot");
        assert!(!hash.is_empty());
        // The workspace itself gained no .git.
        assert!(!ws.path().join(".git").exists());

        // A second snapshot after a change produces a new commit.
        std::fs::write(ws.path().join("keep.txt"), "changed").unwrap();
        let hash2 = take(ws.path()).expect("second snapshot");
        assert_ne!(hash, hash2);
        std::env::remove_var("OXEN_HARNESS_DIR");
    }
}
