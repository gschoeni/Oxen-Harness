//! Detect what a pass actually changed on disk, so conditional gates can skip
//! passes that didn't touch matching files.
//!
//! The check is content-based and git-anchored: before the agent turn we record
//! HEAD plus a content hash of every file that already differs from it; after
//! the turn we diff the working tree against that *same* pre-turn HEAD and
//! compare hashes. Committing existing work moves HEAD but leaves file contents
//! alone, so it correctly reads as "no change". Anything that prevents an
//! answer (not a git repo, git missing, unborn HEAD) yields `None`, and callers
//! treat unknown as changed — failing safe toward running the gates.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

/// A content fingerprint of the dirty part of a git workspace at one moment.
pub struct WorkspaceSnapshot {
    /// The commit the working tree is measured against (HEAD at capture time).
    head: String,
    /// Path → content hash for every file that differed from `head` when
    /// captured. `None` means the path was missing on disk (a delete).
    dirty: HashMap<String, Option<u64>>,
}

/// A file's content relative to the snapshot's anchor commit.
#[derive(PartialEq, Eq)]
enum FileState {
    /// Matches the anchor commit's version.
    Clean,
    /// Differs from the anchor; carries the working-tree content hash
    /// (`None` = missing on disk).
    Dirty(Option<u64>),
}

impl WorkspaceSnapshot {
    /// Fingerprint the workspace now. `None` when change detection is
    /// unavailable (not a git repo, git missing, no commits yet).
    pub fn capture(root: &Path) -> Option<Self> {
        let head = git(root, &["rev-parse", "HEAD"])?.trim().to_string();
        if head.is_empty() {
            return None;
        }
        let mut dirty = HashMap::new();
        for path in status_paths(root)? {
            let hash = hash_file(&root.join(&path));
            dirty.insert(path, hash);
        }
        Some(Self { head, dirty })
    }

    /// The paths whose working-tree content differs from when the snapshot was
    /// captured. Commits alone don't count; edits, creates, deletes, and
    /// reverts do. `None` when git can no longer answer.
    pub fn changed_paths(&self, root: &Path) -> Option<Vec<String>> {
        let mut now_dirty: HashSet<String> = HashSet::new();
        now_dirty.extend(nul_separated(&git(
            root,
            &["diff", "--name-only", "-z", &self.head],
        )?));
        now_dirty.extend(nul_separated(&git(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?));

        let mut candidates: HashSet<&str> = now_dirty.iter().map(String::as_str).collect();
        candidates.extend(self.dirty.keys().map(String::as_str));

        let mut changed: Vec<String> = candidates
            .into_iter()
            .filter(|path| {
                let before = match self.dirty.get(*path) {
                    Some(hash) => FileState::Dirty(*hash),
                    None => FileState::Clean,
                };
                let after = if now_dirty.contains(*path) {
                    FileState::Dirty(hash_file(&root.join(path)))
                } else {
                    FileState::Clean
                };
                before != after
            })
            .map(str::to_string)
            .collect();
        changed.sort();
        Some(changed)
    }
}

/// Run a git command in `root`, returning stdout on success. Change detection
/// treats any git failure the same (fall back to "unknown" → run everything),
/// so this discards the shared runner's error detail into an `Option`.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    harness_core::git::capture(root, args).ok()
}

/// Every path `git status` reports as differing from HEAD (tracked changes,
/// index changes, and untracked files). Rename/copy entries contribute both
/// sides.
fn status_paths(root: &Path) -> Option<Vec<String>> {
    // `-uall` lists untracked files individually (never a collapsed `dir/`),
    // matching how `ls-files --others` reports them after the turn.
    let raw = git(root, &["status", "--porcelain", "-z", "-uall"])?;
    let mut out = Vec::new();
    let mut tokens = raw.split('\0').filter(|t| !t.is_empty());
    while let Some(entry) = tokens.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        out.push(path.to_string());
        // Renames/copies carry the original path as the next NUL token.
        if code.starts_with('R') || code.starts_with('C') {
            if let Some(orig) = tokens.next() {
                out.push(orig.to_string());
            }
        }
    }
    Some(out)
}

fn nul_separated(raw: &str) -> Vec<String> {
    raw.split('\0')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// A cheap content hash for change detection (not security). `None` = the file
/// is missing or unreadable, which itself is a distinguishable state.
fn hash_file(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// Set up a committed git repo in a temp dir; skip the test (return None)
    /// if git isn't available in the environment.
    fn repo() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .ok()
                .filter(|o| o.status.success())
        };
        run(&["init", "-q"])?;
        run(&["config", "user.email", "t@t"])?;
        run(&["config", "user.name", "t"])?;
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "hi\n").unwrap();
        run(&["add", "."])?;
        run(&["commit", "-q", "-m", "init"])?;
        Some((dir, root))
    }

    #[test]
    fn clean_pass_reports_no_changes() {
        let Some((_d, root)) = repo() else { return };
        let snap = WorkspaceSnapshot::capture(&root).unwrap();
        assert_eq!(snap.changed_paths(&root).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn edits_creates_and_deletes_are_detected() {
        let Some((_d, root)) = repo() else { return };
        let snap = WorkspaceSnapshot::capture(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() { edited(); }\n").unwrap();
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap();
        std::fs::remove_file(root.join("README.md")).unwrap();
        let changed = snap.changed_paths(&root).unwrap();
        assert_eq!(changed, vec!["README.md", "a.rs", "new.txt"]);
    }

    #[test]
    fn committing_preexisting_work_is_not_a_change() {
        let Some((_d, root)) = repo() else { return };
        // Dirty the tree *before* the snapshot — like a user asking the agent
        // to "add and commit the changes".
        std::fs::write(root.join("a.rs"), "fn main() { pending(); }\n").unwrap();
        let snap = WorkspaceSnapshot::capture(&root).unwrap();

        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "commit the changes"]);

        assert_eq!(snap.changed_paths(&root).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn further_edits_to_already_dirty_files_are_detected() {
        let Some((_d, root)) = repo() else { return };
        std::fs::write(root.join("a.rs"), "fn main() { pending(); }\n").unwrap();
        let snap = WorkspaceSnapshot::capture(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() { pending(); more(); }\n").unwrap();
        assert_eq!(snap.changed_paths(&root).unwrap(), vec!["a.rs"]);
    }

    #[test]
    fn non_git_directory_yields_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert!(WorkspaceSnapshot::capture(dir.path()).is_none());
    }
}
