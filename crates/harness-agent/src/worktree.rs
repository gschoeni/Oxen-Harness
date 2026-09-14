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

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};

/// One lane's private checkout.
#[derive(Debug)]
pub struct LaneWorktree {
    /// The selected workspace inside the checkout. This may be a repository
    /// subdirectory such as `packages/app`.
    path: PathBuf,
    /// The root Git actually checked out.
    checkout: PathBuf,
    /// The repository the worktree belongs to, for `git worktree remove`.
    repo: PathBuf,
    /// Parent shared by this fleet's lane checkouts. The last lane removes it.
    scratch_root: PathBuf,
    baseline: String,
    preserve: AtomicBool,
}

impl LaneWorktree {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keep a checkout whose changes could not be safely recorded.
    pub fn preserve(&self) {
        self.preserve.store(true, Ordering::Relaxed);
    }

    fn scope(&self) -> &Path {
        self.path
            .strip_prefix(&self.checkout)
            .ok()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
    }
}

impl Drop for LaneWorktree {
    /// Worktrees are process-lifetime scratch space; a crashed run leaves them
    /// for `git worktree prune`, which is exactly what that command is for.
    fn drop(&mut self) {
        if self.preserve.load(Ordering::Relaxed) {
            tracing::warn!(path = %self.path.display(), "preserving unrecorded subagent work");
            return;
        }
        let _ = git(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.checkout.to_string_lossy(),
            ],
        );
        let _ = std::fs::remove_dir(&self.scratch_root);
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

/// Create `count` detached worktrees containing `workspace`, named after
/// `label`.
///
/// Fails if Git cannot create an isolated copy of the current workspace.
pub fn create(workspace: &Path, label: &str, count: usize) -> std::io::Result<Vec<LaneWorktree>> {
    let workspace = workspace.canonicalize()?;
    let repo = repository_root(&workspace)?;
    let prefix = workspace
        .strip_prefix(&repo)
        .map_err(std::io::Error::other)?;
    create_at(&repo, prefix, label, count, "HEAD", true)
}

fn create_at(
    repo: &Path,
    prefix: &Path,
    label: &str,
    count: usize,
    revision: &str,
    carry: bool,
) -> std::io::Result<Vec<LaneWorktree>> {
    // Pid plus a process-wide counter: two fleets running at once (two chat
    // sessions, or a fleet inside a review) must not land on each other's
    // lane directories — the first run's worktrees would be clobbered by the
    // second's, silently.
    static INSTANCE: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "oxen-harness-lanes-{}-{}-{}-{label}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        INSTANCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut lanes = Vec::with_capacity(count);
    for index in 0..count {
        let checkout = root.join(format!("lane-{index}"));
        capture(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                &checkout.to_string_lossy(),
                revision,
            ],
        )?;
        let mut lane = LaneWorktree {
            path: checkout.join(prefix),
            checkout,
            repo: repo.to_path_buf(),
            scratch_root: root.clone(),
            baseline: String::new(),
            preserve: AtomicBool::new(false),
        };
        // `worktree add HEAD` checks out the last commit, not the tree the
        // user is actually looking at. A lane asked to fix code that was just
        // written would not find it, and would return a patch against a state
        // nobody has — so the uncommitted work comes along.
        if carry {
            carry_uncommitted(repo, &lane.checkout, prefix)?;
        }
        std::fs::create_dir_all(&lane.path)?;
        lane.baseline = capture(&lane.checkout, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        lanes.push(lane);
    }
    Ok(lanes)
}

/// Reproduce the parent's uncommitted state in a fresh worktree: tracked
/// modifications as a patch, then untracked (non-ignored) files copied, then a
/// baseline commit so what the lane later reports is the lane's own work
/// rather than the user's.
///
/// The commit lands on the worktree's detached HEAD, so no branch in the
/// parent repository is touched.
///
/// Any copy failure aborts construction so a lane never starts from stale files.
fn carry_uncommitted(repo: &Path, checkout: &Path, prefix: &Path) -> std::io::Result<()> {
    let scope = pathspec(prefix);
    let diff = capture(repo, &["diff", "HEAD", "--binary", "--", &scope])?;
    if !diff.trim().is_empty() {
        let patch = checkout.join(".oxen-harness-carry.patch");
        std::fs::write(&patch, &diff)?;
        let applied = capture(checkout, &["apply", &patch.to_string_lossy()]);
        std::fs::remove_file(&patch)?;
        applied?;
    }
    let untracked = capture(
        repo,
        &[
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "--full-name",
            "--",
            &scope,
        ],
    )?;
    for rel in untracked.split('\0').filter(|l| !l.is_empty()) {
        let (from, to) = (repo.join(rel), checkout.join(rel));
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        if from.symlink_metadata()?.file_type().is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
            continue;
        }
        std::fs::copy(&from, &to)?;
    }
    baseline(checkout, prefix)
}

/// Commit whatever the lane starts with, so `changes` reports only what the
/// lane did. Identity is supplied inline: a repository without `user.email`
/// configured would otherwise refuse the commit and every lane would report
/// the user's own edits as its own.
fn baseline(checkout: &Path, prefix: &Path) -> std::io::Result<()> {
    let scope = pathspec(prefix);
    capture(checkout, &["add", "-A", "--", &scope])?;
    capture(
        checkout,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.email=agent@oxen-harness.local",
            "-c",
            "user.name=oxen-harness",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "lane baseline (the working tree this lane started from)",
        ],
    )?;
    Ok(())
}

/// What a lane changed in its worktree, or `None` when it changed nothing.
///
/// Untracked files are staged first so new files appear in the patch — a lane
/// that adds a module and never mentions it would otherwise report "no
/// changes" while its work sat invisible in a temp directory.
pub fn changes(lane: &LaneWorktree) -> std::io::Result<Option<LaneChanges>> {
    let scope = lane.scope().to_string_lossy();
    capture(&lane.checkout, &["add", "-A", "--", &scope])?;
    let summary = capture(
        &lane.checkout,
        &["diff", "--cached", &lane.baseline, "--stat", "--", &scope],
    )?;
    if summary.trim().is_empty() {
        return Ok(None);
    }
    let patch = capture(
        &lane.checkout,
        &["diff", "--cached", &lane.baseline, "--binary", "--", &scope],
    )?;
    Ok(Some(LaneChanges {
        patch,
        summary: summary.trim().to_string(),
    }))
}

fn repository_root(path: &Path) -> std::io::Result<PathBuf> {
    let root = capture(path, &["rev-parse", "--show-toplevel"])?;
    PathBuf::from(root.trim()).canonicalize()
}

fn pathspec(prefix: &Path) -> std::borrow::Cow<'_, str> {
    if prefix.as_os_str().is_empty() {
        std::borrow::Cow::Borrowed(".")
    } else {
        prefix.to_string_lossy()
    }
}

fn git(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .is_ok_and(|out| out.status.success())
}

fn capture(cwd: &Path, args: &[&str]) -> std::io::Result<String> {
    let out = Command::new("git").args(args).current_dir(cwd).output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    String::from_utf8(out.stdout).map_err(std::io::Error::other)
}

/// A recoverable isolated workspace. The ref pins the baseline across Git GC;
/// the patch is stored with the lane, independent of the overflow cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeSnapshot {
    pub repository: PathBuf,
    pub prefix: PathBuf,
    pub baseline_ref: String,
    pub patch: String,
}

impl WorktreeSnapshot {
    pub fn capture(lane: &LaneWorktree, id: &str) -> std::io::Result<Self> {
        let repository = PathBuf::from(
            capture(
                &lane.checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )?
            .trim(),
        );
        let baseline_ref = format!("refs/oxen-harness/lanes/{id}");
        capture(&repository, &["update-ref", &baseline_ref, &lane.baseline])?;
        Ok(Self {
            repository,
            prefix: lane
                .path
                .strip_prefix(&lane.checkout)
                .map_err(std::io::Error::other)?
                .to_path_buf(),
            baseline_ref,
            patch: changes(lane)?.map(|c| c.patch).unwrap_or_default(),
        })
    }

    pub fn restore(&self) -> std::io::Result<LaneWorktree> {
        let mut lanes = create_at(
            &self.repository,
            &self.prefix,
            "resume",
            1,
            &self.baseline_ref,
            false,
        )?;
        let lane = lanes.pop().ok_or_else(|| {
            std::io::Error::other("could not restore agent workspace: no checkout was created")
        })?;
        if !self.patch.is_empty() {
            let file = lane.scratch_root.join("restore.patch");
            std::fs::write(&file, &self.patch)?;
            capture(&lane.checkout, &["apply", &file.to_string_lossy()])?;
            std::fs::remove_file(file)?;
        }
        Ok(lane)
    }
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

        let changes = changes(&lanes[0]).unwrap().expect("changes");

        assert!(changes.patch.contains("changed()"), "{}", changes.patch);
        // A new file the lane never mentions must not vanish silently.
        assert!(changes.patch.contains("added.rs"), "{}", changes.patch);
        assert!(changes.summary.contains("main.rs"), "{}", changes.summary);
    }

    #[test]
    fn a_subdirectory_workspace_keeps_its_repository_prefix() {
        let dir = repo();
        let app = dir.path().join("packages/app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("lib.rs"), "pub fn app() {}\n").unwrap();
        assert!(git(dir.path(), &["add", "-A"]));
        assert!(git(dir.path(), &["commit", "-qm", "add app"]));
        std::fs::write(app.join("scratch.rs"), "pub fn scratch() {}\n").unwrap();

        let lanes = create(&app, "nested", 1).expect("nested worktree");
        let lane = &lanes[0];

        assert!(
            lane.path().ends_with("packages/app"),
            "selected prefix lost: {}",
            lane.path().display()
        );
        assert_eq!(
            std::fs::read_to_string(lane.path().join("lib.rs")).unwrap(),
            "pub fn app() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(lane.path().join("scratch.rs")).unwrap(),
            "pub fn scratch() {}\n",
            "subdirectory-relative untracked files belong under the prefix"
        );
        assert!(
            !lane.path().join("main.rs").exists(),
            "the repository root must not be exposed as the lane workspace"
        );
    }

    #[test]
    fn binary_changes_are_carried_in_the_returned_patch() {
        let dir = repo();
        let lanes = create(dir.path(), "binary", 1).unwrap();
        std::fs::write(
            lanes[0].path().join("image.bin"),
            [0_u8, 159, 146, 150, 0, 255],
        )
        .unwrap();

        let changes = changes(&lanes[0]).unwrap().expect("binary change");

        assert!(
            changes.patch.contains("GIT binary patch"),
            "{}",
            changes.patch
        );
        assert!(changes.patch.contains("image.bin"), "{}", changes.patch);
    }

    #[test]
    fn a_lane_starts_from_the_tree_the_user_is_looking_at() {
        let dir = repo();
        // The state a fleet is usually spawned into: edited and new files that
        // were never committed.
        std::fs::write(
            dir.path().join("main.rs"),
            "fn main() { work_in_progress() }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("scratch.rs"), "pub fn added() {}\n").unwrap();

        let lanes = create(dir.path(), "test", 1).expect("worktrees");

        assert_eq!(
            std::fs::read_to_string(lanes[0].path().join("main.rs")).unwrap(),
            "fn main() { work_in_progress() }\n",
            "the lane should see uncommitted edits"
        );
        assert_eq!(
            std::fs::read_to_string(lanes[0].path().join("scratch.rs")).unwrap(),
            "pub fn added() {}\n",
            "the lane should see untracked files"
        );
        // And the carrier patch must not be left behind as a change of its own.
        assert!(
            changes(&lanes[0]).unwrap().is_none(),
            "a fresh lane has changed nothing"
        );
    }

    #[test]
    fn a_lane_that_changed_nothing_reports_nothing() {
        let dir = repo();
        let lanes = create(dir.path(), "test", 1).unwrap();
        assert!(changes(&lanes[0]).unwrap().is_none());
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
        assert!(create(dir.path(), "test", 1).is_err());
    }
}
