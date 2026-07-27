//! Workspace files — the Files tree and the Editor pane browse, read, and
//! write files inside a chat's working directory. Every command takes the
//! workspace root (the frontend knows it from the session) plus a
//! workspace-relative path, and refuses anything that would escape the root,
//! so the webview can never reach outside the project it's showing.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// One row in the Files tree.
#[derive(Clone, Serialize)]
pub(crate) struct FileEntry {
    name: String,
    /// Workspace-relative path, `/`-joined — the tree's stable key.
    path: String,
    is_dir: bool,
}

/// A text file's content for the editor.
#[derive(Clone, Serialize)]
pub(crate) struct FileBody {
    content: String,
    /// True when the file was longer than the read cap and was cut off —
    /// the editor opens read-only so a save can't destroy the tail.
    truncated: bool,
    size: u64,
}

/// A changed path in the workspace's Git working tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GitFileState {
    /// Workspace-relative path, `/`-joined (matches the Files tree's keys).
    path: String,
    /// Where a rename came from, when Git reports one.
    original_path: Option<String>,
    /// VS Code-style summary: modified, added, deleted, renamed, untracked,
    /// or conflicted.
    status: String,
    /// Git's index status letter (space when clean).
    index: String,
    /// Git's working-tree status letter (space when clean).
    worktree: String,
}

/// A unified diff for one changed file.
#[derive(Clone, Serialize)]
pub(crate) struct GitFileDiff {
    content: String,
    truncated: bool,
}

/// Editor/diff read cap. Files beyond this open truncated + read-only;
/// anything that big is a build artifact or a dataset, not something to
/// hand-edit or eyeball as a diff.
const MAX_READ_BYTES: usize = 2_000_000;

/// Run git in the workspace root. `Err` is reserved for "couldn't even run
/// git" (not installed); a non-zero exit comes back in the `Output` for the
/// caller to interpret (most commonly: not a repository).
fn run_git(root: &str, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))
}

/// Map porcelain status letters (index, worktree) to the VS Code-style
/// summary word the UI shows.
fn summarize_status(index: char, worktree: char) -> &'static str {
    match (index, worktree) {
        ('?', '?') => "untracked",
        ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => "conflicted",
        ('R', _) | (_, 'R') => "renamed",
        ('A', _) => "added",
        ('D', _) | (_, 'D') => "deleted",
        _ => "modified",
    }
}

/// The workspace's changed files, VS Code Source-Control style: one entry per
/// path with its porcelain letters and a summary status. `None` when the
/// workspace isn't a Git repository (or git isn't installed) — the UI hides
/// the whole section rather than showing an error.
#[tauri::command]
pub(crate) fn git_status(root: String) -> Result<Option<Vec<GitFileState>>, String> {
    // Same validation as every other command: only ever inspect a real
    // workspace root the frontend legitimately holds.
    resolve(&root, "")?;
    let Ok(out) = run_git(
        &root,
        // -z: NUL-separated, no quoting/escaping to undo. -uall: every
        // untracked file individually, not collapsed directories.
        &["status", "--porcelain", "-z", "-uall"],
    ) else {
        return Ok(None);
    };
    if !out.status.success() {
        return Ok(None);
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let mut fields = raw.split('\0');
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        // "XY path" — the two status letters, a space, then the path.
        if field.len() < 4 {
            continue;
        }
        let mut chars = field.chars();
        let index = chars.next().unwrap_or(' ');
        let worktree = chars.next().unwrap_or(' ');
        let path = field[3..].to_string();
        // A rename carries the source path as its own NUL field right after.
        let original_path = if index == 'R' || worktree == 'R' {
            fields.next().map(str::to_string)
        } else {
            None
        };
        entries.push(GitFileState {
            path,
            original_path,
            status: summarize_status(index, worktree).to_string(),
            index: index.to_string(),
            worktree: worktree.to_string(),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some(entries))
}

/// The unified diff for one changed file: working tree vs HEAD, so staged and
/// unstaged edits show as one change (the app has no staging UI). Untracked
/// files diff against nothing, so the whole file reads as added.
#[tauri::command]
pub(crate) fn git_diff(root: String, path: String) -> Result<GitFileDiff, String> {
    resolve(&root, &path)?;
    // Tracked or not decides which diff exists for this path.
    let tracked = run_git(&root, &["ls-files", "--error-unmatch", "--", &path])
        .map(|out| out.status.success())
        .unwrap_or(false);
    let out = if tracked {
        let head = run_git(&root, &["diff", "HEAD", "--", &path])?;
        if head.status.success() {
            head
        } else {
            // No HEAD yet (unborn branch): everything tracked is newly staged.
            run_git(&root, &["diff", "--cached", "--", &path])?
        }
    } else {
        // --no-index exits 1 when the files differ — that's the diff, not an
        // error.
        run_git(&root, &["diff", "--no-index", "--", "/dev/null", &path])?
    };
    let diff = String::from_utf8_lossy(&out.stdout);
    if diff.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.is_empty() {
            return Err(format!("git diff failed for {path}: {}", err.trim()));
        }
    }
    let truncated = diff.len() > MAX_READ_BYTES;
    let content = if truncated {
        let mut cut = MAX_READ_BYTES;
        while cut > 0 && !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff[..cut].to_string()
    } else {
        diff.into_owned()
    };
    Ok(GitFileDiff { content, truncated })
}

/// Join `rel` onto `root`, refusing absolute paths and any `..` step so the
/// result provably stays inside the workspace.
pub(super) fn resolve(root: &str, rel: &str) -> Result<PathBuf, String> {
    let root_path = Path::new(root);
    if !root_path.is_absolute() || !root_path.is_dir() {
        return Err(format!("not a workspace directory: {root}"));
    }
    let mut resolved = root_path.to_path_buf();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(seg) => resolved.push(seg),
            Component::CurDir => {}
            _ => return Err(format!("path escapes the workspace: {rel}")),
        }
    }
    Ok(resolved)
}

/// List one directory of the workspace tree (the tree loads lazily, a level
/// per expand). Directories first, then files, each alphabetical. `.git` is
/// the one thing hidden — it's plumbing, not project content.
#[tauri::command]
pub(crate) fn fs_list_dir(root: String, path: String) -> Result<Vec<FileEntry>, String> {
    let dir = resolve(&root, &path)?;
    let read = fs::read_dir(&dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
    let mut entries: Vec<FileEntry> = read
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                return None;
            }
            let is_dir = entry.file_type().ok()?.is_dir();
            let rel = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            Some(FileEntry {
                name,
                path: rel,
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Read a text file for the editor. Binary content is refused (the viewer
/// shows images/videos natively instead); oversized files come back truncated.
#[tauri::command]
pub(crate) fn fs_read_file(root: String, path: String) -> Result<FileBody, String> {
    let file = resolve(&root, &path)?;
    let meta = fs::metadata(&file).map_err(|e| format!("could not open {path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a file: {path}"));
    }
    let bytes = fs::read(&file).map_err(|e| format!("could not read {path}: {e}"))?;
    let truncated = bytes.len() > MAX_READ_BYTES;
    let slice = if truncated {
        &bytes[..MAX_READ_BYTES]
    } else {
        &bytes[..]
    };
    // A truncated read may split a UTF-8 sequence at the cut; trim to the last
    // complete character rather than calling the whole file binary.
    let content = match std::str::from_utf8(slice) {
        Ok(text) => text.to_string(),
        Err(e) if truncated && slice.len() - e.valid_up_to() < 4 => {
            std::str::from_utf8(&slice[..e.valid_up_to()])
                .unwrap_or_default()
                .to_string()
        }
        Err(_) => return Err(format!("{path} is a binary file")),
    };
    Ok(FileBody {
        content,
        truncated,
        size: meta.len(),
    })
}

/// Save the editor's buffer back to disk.
#[tauri::command]
pub(crate) fn fs_write_file(root: String, path: String, content: String) -> Result<(), String> {
    let file = resolve(&root, &path)?;
    fs::write(&file, content).map_err(|e| format!("could not save {path}: {e}"))
}

/// Create an empty file or a directory. Fails if something already exists at
/// the path, so a typo can't silently truncate a real file.
#[tauri::command]
pub(crate) fn fs_create_entry(root: String, path: String, is_dir: bool) -> Result<(), String> {
    let target = resolve(&root, &path)?;
    if target.exists() {
        return Err(format!("{path} already exists"));
    }
    if is_dir {
        fs::create_dir(&target).map_err(|e| format!("could not create folder {path}: {e}"))
    } else {
        fs::File::create_new(&target)
            .map(|_| ())
            .map_err(|e| format!("could not create {path}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        // Unique per test — the tests run concurrently in one process.
        let dir =
            std::env::temp_dir().join(format!("oxen-harness-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("README.md"), "hello").unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        dir
    }

    #[test]
    fn listing_hides_git_and_sorts_directories_first() {
        let dir = workspace("list");
        let root = dir.display().to_string();
        let entries = fs_list_dir(root.clone(), String::new()).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["src", "README.md"]);
        let nested = fs_list_dir(root, "src".into()).unwrap();
        assert_eq!(nested[0].path, "src/main.rs");
        fs::remove_dir_all(dir).unwrap();
    }

    /// Turn a workspace into a git repository with one commit.
    fn git_init(dir: &Path) {
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .args(args)
                    .output()
                    .expect("git runs")
                    .status
                    .success(),
                "git {args:?} failed"
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
    }

    #[test]
    fn git_status_reports_the_working_tree_vscode_style() {
        let dir = workspace("gitstatus");
        // Not a repository yet: the section simply doesn't exist.
        assert_eq!(git_status(dir.display().to_string()).unwrap(), None);

        git_init(&dir);
        fs::write(dir.join("README.md"), "changed").unwrap(); // modified
        fs::write(dir.join("new.txt"), "fresh").unwrap(); // untracked
        fs::remove_file(dir.join("src/main.rs")).unwrap(); // deleted

        let states = git_status(dir.display().to_string()).unwrap().unwrap();
        let by_path: std::collections::HashMap<_, _> =
            states.iter().map(|s| (s.path.as_str(), s)).collect();
        assert_eq!(by_path["README.md"].status, "modified");
        assert_eq!(by_path["new.txt"].status, "untracked");
        assert_eq!(by_path["new.txt"].index, "?");
        assert_eq!(by_path["src/main.rs"].status, "deleted");
        // Sorted by path, so the UI's list is stable across refreshes.
        let paths: Vec<_> = states.iter().map(|s| s.path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);

        // A clean tree is an empty list, not None.
        fs::remove_file(dir.join("new.txt")).unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["checkout", "--", "."])
            .output()
            .unwrap();
        assert_eq!(
            git_status(dir.display().to_string()).unwrap(),
            Some(Vec::new())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn git_status_follows_renames() {
        let dir = workspace("gitrename");
        git_init(&dir);
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["mv", "README.md", "MOVED.md"]);
        let states = git_status(dir.display().to_string()).unwrap().unwrap();
        let renamed = states.iter().find(|s| s.path == "MOVED.md").unwrap();
        assert_eq!(renamed.status, "renamed");
        assert_eq!(renamed.original_path.as_deref(), Some("README.md"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn git_diff_covers_modified_untracked_and_deleted() {
        let dir = workspace("gitdiff");
        git_init(&dir);
        let root = dir.display().to_string();

        fs::write(dir.join("README.md"), "changed\n").unwrap();
        let diff = git_diff(root.clone(), "README.md".into()).unwrap();
        assert!(diff.content.contains("-hello"), "{}", diff.content);
        assert!(diff.content.contains("+changed"), "{}", diff.content);
        assert!(!diff.truncated);

        // Untracked: the whole file reads as added.
        fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let diff = git_diff(root.clone(), "new.txt".into()).unwrap();
        assert!(diff.content.contains("+fresh"), "{}", diff.content);

        // Deleted: the whole file reads as removed.
        fs::remove_file(dir.join("src/main.rs")).unwrap();
        let diff = git_diff(root.clone(), "src/main.rs".into()).unwrap();
        assert!(diff.content.contains("-fn main()"), "{}", diff.content);

        // Diffs respect the same escape rules as every file command.
        assert!(git_diff(root, "../etc/passwd".into()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn paths_cannot_escape_the_workspace() {
        let dir = workspace("escape");
        let root = dir.display().to_string();
        assert!(fs_read_file(root.clone(), "../etc/passwd".into()).is_err());
        assert!(fs_read_file(root.clone(), "/etc/passwd".into()).is_err());
        assert!(fs_create_entry(root, "../oops".into(), true).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round_trips_edits_and_refuses_existing_targets() {
        let dir = workspace("roundtrip");
        let root = dir.display().to_string();
        fs_write_file(root.clone(), "README.md".into(), "updated".into()).unwrap();
        let body = fs_read_file(root.clone(), "README.md".into()).unwrap();
        assert_eq!(body.content, "updated");
        assert!(!body.truncated);
        fs_create_entry(root.clone(), "notes".into(), true).unwrap();
        fs_create_entry(root.clone(), "notes/todo.md".into(), false).unwrap();
        assert!(fs_create_entry(root.clone(), "README.md".into(), false).is_err());
        // Binary content is refused rather than mangled.
        fs::write(dir.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        assert!(fs_read_file(root, "blob.bin".into()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
