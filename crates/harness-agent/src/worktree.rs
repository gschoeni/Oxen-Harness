//! Git worktrees for fleet lanes that write.
//!
//! `spawn_agents` hands every lane the same registry, rooted at the same
//! workspace. That is fine for lanes that read — the point of fanning out — but
//! it makes a fleet that *edits* a footgun: N lanes doing read-modify-write on
//! one tree produce interleaved, mutually inconsistent changes, and the
//! per-path locking added to the fs tools only makes each individual write
//! atomic, not the set of them coherent.
//!
//! So an editing fleet asks for isolation: each lane gets a detached `git
//! worktree` of HEAD, its file and shell tools rooted there, and its changes
//! come back as a patch the parent can read and apply deliberately. Nothing is
//! merged automatically — the parent agent decides what to keep, which is the
//! whole reason to isolate rather than to serialize.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// One lane's private checkout.
#[derive(Debug)]
pub struct LaneWorktree {
    path: PathBuf,
    /// The repository the worktree belongs to, for `git worktree remove`.
    repo: PathBuf,
}

impl LaneWorktree {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LaneWorktree {
    /// Worktrees are process-lifetime scratch space; a crashed run leaves them
    /// for `git worktree prune`, which is exactly what that command is for.
    fn drop(&mut self) {
        let _ = git(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ],
        );
    }
}

/// The changes a lane made, as a patch plus a one-line summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneChanges {
    /// `git diff` output covering tracked edits and newly added files.
    pub patch: String,
    /// `git diff --stat`-style summary for the model to read at a glance.
    pub summary: String,
}

/// Create `count` detached worktrees of `repo`'s HEAD, named after `label`.
///
/// Returns `None` when the workspace isn't a git repository or git refuses —
/// callers fall back to the shared workspace rather than failing the fleet,
/// since isolation is a safety upgrade, not a precondition.
pub fn create(repo: &Path, label: &str, count: usize) -> Option<Vec<LaneWorktree>> {
    if !is_git_repo(repo) {
        return None;
    }
    // Pid plus a process-wide counter: two fleets running at once (two chat
    // sessions, or a fleet inside a review) must not land on each other's
    // lane directories — the first run's worktrees would be clobbered by the
    // second's, silently.
    static INSTANCE: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "oxen-harness-lanes-{}-{}-{label}",
        std::process::id(),
        INSTANCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut lanes = Vec::with_capacity(count);
    for index in 0..count {
        let path = root.join(format!("lane-{index}"));
        // A stale directory from a crashed run would make `worktree add` fail.
        let _ = std::fs::remove_dir_all(&path);
        let added = git(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                &path.to_string_lossy(),
                "HEAD",
            ],
        );
        if !added {
            // Partial success is worse than none: the lanes that did get a
            // worktree are dropped (and removed) with `lanes`.
            return None;
        }
        lanes.push(LaneWorktree {
            path,
            repo: repo.to_path_buf(),
        });
    }
    Some(lanes)
}

/// What a lane changed in its worktree, or `None` when it changed nothing.
///
/// Untracked files are staged first so new files appear in the patch — a lane
/// that adds a module and never mentions it would otherwise report "no
/// changes" while its work sat invisible in a temp directory.
pub fn changes(lane: &LaneWorktree) -> Option<LaneChanges> {
    let _ = git(&lane.path, &["add", "-A"]);
    let summary = capture(&lane.path, &["diff", "--cached", "--stat"])?;
    if summary.trim().is_empty() {
        return None;
    }
    let patch = capture(&lane.path, &["diff", "--cached"])?;
    Some(LaneChanges {
        patch,
        summary: summary.trim().to_string(),
    })
}

fn is_git_repo(path: &Path) -> bool {
    capture(path, &["rev-parse", "--git-dir"]).is_some()
}

fn git(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .is_ok_and(|out| out.status.success())
}

fn capture(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with one commit — `worktree add HEAD` needs a commit.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(git(root, &["init", "-q"]));
        assert!(git(root, &["config", "user.email", "test@example.com"]));
        assert!(git(root, &["config", "user.name", "Test"]));
        std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
        assert!(git(root, &["add", "-A"]));
        assert!(git(root, &["commit", "-qm", "init"]));
        dir
    }

    #[test]
    fn lanes_get_independent_checkouts_of_head() {
        let dir = repo();
        let lanes = create(dir.path(), "test", 2).expect("worktrees");

        assert_eq!(lanes.len(), 2);
        for lane in &lanes {
            assert_eq!(
                std::fs::read_to_string(lane.path().join("main.rs")).unwrap(),
                "fn main() {}\n"
            );
        }
        // Editing one lane leaves the other — and the parent — untouched.
        std::fs::write(lanes[0].path().join("main.rs"), "fn main() { one() }\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(lanes[1].path().join("main.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("main.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn a_lanes_edits_come_back_as_a_patch() {
        let dir = repo();
        let lanes = create(dir.path(), "test", 1).unwrap();
        std::fs::write(lanes[0].path().join("main.rs"), "fn main() { changed() }\n").unwrap();
        std::fs::write(lanes[0].path().join("added.rs"), "pub fn extra() {}\n").unwrap();

        let changes = changes(&lanes[0]).expect("changes");

        assert!(changes.patch.contains("changed()"), "{}", changes.patch);
        // A new file the lane never mentions must not vanish silently.
        assert!(changes.patch.contains("added.rs"), "{}", changes.patch);
        assert!(changes.summary.contains("main.rs"), "{}", changes.summary);
    }

    #[test]
    fn a_lane_that_changed_nothing_reports_nothing() {
        let dir = repo();
        let lanes = create(dir.path(), "test", 1).unwrap();
        assert!(changes(&lanes[0]).is_none());
    }

    #[test]
    fn a_worktree_is_removed_when_its_lane_drops() {
        let dir = repo();
        let path = {
            let lanes = create(dir.path(), "test", 1).unwrap();
            lanes[0].path().to_path_buf()
        };
        assert!(!path.exists(), "the worktree should be cleaned up");
    }

    #[test]
    fn a_directory_that_is_not_a_repository_declines_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(create(dir.path(), "test", 1).is_none());
    }
}
