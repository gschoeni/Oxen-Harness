//! One place to shell out to `git` and capture stdout.
//!
//! Several crates need to run a git command in a working directory and read
//! its output — the review pipeline computing a diff, the loop detecting what a
//! pass changed, the CLI checking whether a ref exists. Each had grown its own
//! copy with a slightly different error/stderr policy; this is the shared
//! runner they call instead, so the behavior (and the `git … failed: <stderr>`
//! message shape) is defined once. It depends only on `std`, so it belongs in
//! the leaf crate alongside the other cross-cutting helpers.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::bounded::BoundedText;

const MAX_CAPTURE_CHARS: usize = 100_000;

/// Run `git <args>` in `root`, returning captured stdout on success.
///
/// On failure the error carries the command and git's trimmed stderr, so a
/// caller can surface *why* it failed; callers that only care whether it
/// succeeded can `.ok()` the result. Never panics — a missing `git` binary is
/// an `Err`, not a crash.
pub fn capture(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run git: {e}"))?;
    let stdout = child.stdout.take().map(|s| drain(s, MAX_CAPTURE_CHARS));
    let stderr = child.stderr.take().map(|s| drain(s, MAX_CAPTURE_CHARS));
    let status = child
        .wait()
        .map_err(|e| format!("could not wait for git: {e}"))?;
    let stdout = stdout.map(join_reader).unwrap_or_default();
    let stderr = stderr.map(join_reader).unwrap_or_default();
    if !status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(stdout)
}

/// A glanceable summary of a workspace's git state: what branch it's on and
/// how far it has drifted from clean/pushed. This is the workspace-level truth
/// an overview surface (the Ledger's project banner) renders — deliberately
/// per-*workspace*, not per-session, because git cannot attribute a dirty tree
/// or unpushed commits to any one conversation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitOverview {
    /// The checked-out branch, or a short commit sha when HEAD is detached,
    /// e.g. during a rebase. Empty only if git reports nothing at all.
    pub branch: String,
    /// Files changed relative to HEAD (staged, unstaged, and untracked).
    pub dirty_files: usize,
    /// Commits on HEAD that the upstream doesn't have. 0 when `has_upstream`
    /// is false — "no upstream" and "fully pushed" are different stories, so
    /// the flag travels alongside rather than being folded into the counts.
    pub ahead: usize,
    /// Commits on the upstream that HEAD doesn't have.
    pub behind: usize,
    /// Whether the current branch tracks an upstream at all.
    pub has_upstream: bool,
}

/// Summarize the git state of `root`, or `None` when it isn't inside a git
/// work tree (or git isn't installed). Never errors: an overview is decoration
/// on top of a workspace, so anything short of an answer degrades to `None`
/// or to zeroed fields rather than failing the caller.
pub fn overview(root: &Path) -> Option<GitOverview> {
    // `status --porcelain` doubles as the is-this-a-repo gate: it fails
    // outside a work tree and succeeds (possibly empty) everywhere else,
    // including a freshly `git init`ed repo with no commits.
    let status = capture(root, &["status", "--porcelain"]).ok()?;
    let dirty_files = status.lines().filter(|l| !l.trim().is_empty()).count();

    // Branch name via symbolic-ref (quiet, fails when detached); fall back to
    // the short sha so a mid-rebase workspace still labels itself honestly.
    let branch = capture(root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .or_else(|_| capture(root, &["rev-parse", "--short", "HEAD"]))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // `HEAD...@{upstream}` counts divergence in both directions in one call:
    // "<ahead>\t<behind>". It fails when no upstream is configured (and on an
    // unborn HEAD), which is exactly the `has_upstream: false` story.
    let (ahead, behind, has_upstream) = match capture(
        root,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    ) {
        Ok(counts) => {
            let mut parts = counts.split_whitespace();
            let ahead = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let behind = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            (ahead, behind, true)
        }
        Err(_) => (0, 0, false),
    };

    Some(GitOverview {
        branch,
        dirty_files,
        ahead,
        behind,
        has_upstream,
    })
}

fn drain(
    mut reader: impl Read + Send + 'static,
    max_chars: usize,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut kept = BoundedText::new(max_chars);
        let mut bytes = [0u8; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => kept.push(&String::from_utf8_lossy(&bytes[..n])),
            }
        }
        kept.into_string()
    })
}

fn join_reader(handle: std::thread::JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_failure_with_stderr() {
        // `git` in a non-repo directory fails; the error names the command.
        let tmp = std::env::temp_dir();
        let err = capture(&tmp, &["rev-parse", "--absurd-flag-xyz"]).unwrap_err();
        assert!(err.starts_with("git rev-parse"), "{err}");
    }

    #[test]
    fn captures_stdout_of_a_trivial_command() {
        // `git --version` succeeds anywhere git is installed; skip if absent.
        let tmp = std::env::temp_dir();
        if let Ok(out) = capture(&tmp, &["--version"]) {
            assert!(out.contains("git"));
        }
    }

    /// A throwaway repo (or `None` when git is unavailable in the sandbox).
    fn scratch_repo() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        capture(dir.path(), &["init", "-q", "-b", "main"]).ok()?;
        capture(dir.path(), &["config", "user.email", "t@e.st"]).ok()?;
        capture(dir.path(), &["config", "user.name", "t"]).ok()?;
        Some(dir)
    }

    #[test]
    fn overview_is_none_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        if capture(dir.path(), &["--version"]).is_ok() {
            assert!(overview(dir.path()).is_none());
        }
    }

    #[test]
    fn overview_reads_branch_dirt_and_upstream_absence() {
        let Some(dir) = scratch_repo() else { return };
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let ov = overview(dir.path()).unwrap();
        assert_eq!(ov.branch, "main");
        assert_eq!(ov.dirty_files, 1);
        assert!(!ov.has_upstream);
        assert_eq!((ov.ahead, ov.behind), (0, 0));

        // Committing cleans the tree; still no upstream to be ahead of.
        capture(dir.path(), &["add", "-A"]).unwrap();
        capture(dir.path(), &["commit", "-q", "-m", "one"]).unwrap();
        let ov = overview(dir.path()).unwrap();
        assert_eq!(ov.dirty_files, 0);
        assert!(!ov.has_upstream);
    }

    #[test]
    fn overview_counts_commits_ahead_of_upstream() {
        let Some(dir) = scratch_repo() else { return };
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        capture(dir.path(), &["add", "-A"]).unwrap();
        capture(dir.path(), &["commit", "-q", "-m", "one"]).unwrap();
        // A local branch tracking another local branch is upstream enough.
        capture(dir.path(), &["branch", "base"]).unwrap();
        capture(dir.path(), &["branch", "-u", "base"]).unwrap();
        std::fs::write(dir.path().join("b.txt"), "yo").unwrap();
        capture(dir.path(), &["add", "-A"]).unwrap();
        capture(dir.path(), &["commit", "-q", "-m", "two"]).unwrap();

        let ov = overview(dir.path()).unwrap();
        assert!(ov.has_upstream);
        assert_eq!((ov.ahead, ov.behind), (1, 0));
    }
}
