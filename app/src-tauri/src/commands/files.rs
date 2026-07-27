//! Workspace files — the Files tree and the Editor pane browse, read, and
//! write files inside a chat's working directory. Every command takes the
//! workspace root (the frontend knows it from the session) plus a
//! workspace-relative path, and refuses anything that would escape the root,
//! so the webview can never reach outside the project it's showing.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

/// How long a viewer git command may run before it's killed — a diff for a
/// UI pane is worthless after this long anyway.
const GIT_VIEW_TIMEOUT: Duration = Duration::from_secs(20);
/// The most stderr the viewer keeps for an error message.
const MAX_STDERR_BYTES: usize = 8 * 1024;

/// A git command's output with its stdout hard-capped DURING the read.
struct CappedGit {
    stdout: Vec<u8>,
    stderr: String,
    truncated: bool,
    success: bool,
}

/// Run git streaming stdout with a hard byte cap and a wall-clock timeout.
///
/// `Command::output()` would buffer the ENTIRE stream before any truncation
/// could apply — a diff of a huge generated file could take the desktop
/// process down with it. Here the reader stops at the cap (dropping the pipe,
/// which ends git with EPIPE), and a deadline kills a wedged child.
fn run_git_capped(root: &str, args: &[&str], cap: usize) -> Result<CappedGit, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run git: {e}"))?;

    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 64 * 1024];
        loop {
            match stdout_pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(out.len());
                    out.extend_from_slice(&buf[..n.min(room)]);
                    if n > room {
                        truncated = true;
                        break; // dropping the pipe ends the child's stream
                    }
                }
            }
        }
        (out, truncated)
    });
    let err_reader = std::thread::spawn(move || {
        let mut err = Vec::new();
        let _ = (&mut stderr_pipe)
            .take(MAX_STDERR_BYTES as u64)
            .read_to_end(&mut err);
        // Drain the rest so a chatty child can't block on a full pipe.
        let _ = std::io::copy(&mut stderr_pipe, &mut std::io::sink());
        String::from_utf8_lossy(&err).into_owned()
    });

    let deadline = Instant::now() + GIT_VIEW_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let (stdout, truncated) = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(format!(
            "git {} did not finish within {}s",
            args.join(" "),
            GIT_VIEW_TIMEOUT.as_secs()
        ));
    };
    Ok(CappedGit {
        stdout,
        stderr,
        truncated,
        // A cap-truncated child dies of EPIPE — that's our doing, not a failure.
        success: status.success() || truncated,
    })
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
    // The viewer must never execute repository-configured programs: a repo's
    // .gitattributes/config can attach external diff drivers and textconv
    // helpers, and "look at a diff" must not run them.
    const SAFE_DIFF: &[&str] = &["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    fn args<'a>(rest: &[&'a str]) -> Vec<&'a str> {
        [SAFE_DIFF, rest].concat()
    }
    // Tracked or not decides which diff exists for this path.
    let tracked = run_git(&root, &["ls-files", "--error-unmatch", "--", &path])
        .map(|out| out.status.success())
        .unwrap_or(false);
    let out = if tracked {
        let head = run_git_capped(&root, &args(&["HEAD", "--", &path]), MAX_READ_BYTES)?;
        if head.success {
            head
        } else {
            // No HEAD yet (unborn branch): everything tracked is newly staged.
            run_git_capped(&root, &args(&["--cached", "--", &path]), MAX_READ_BYTES)?
        }
    } else {
        // --no-index exits 1 when the files differ — that's the diff, not an
        // error.
        run_git_capped(
            &root,
            &args(&["--no-index", "--", "/dev/null", &path]),
            MAX_READ_BYTES,
        )?
    };
    if out.stdout.is_empty() && !out.stderr.is_empty() {
        return Err(format!("git diff failed for {path}: {}", out.stderr.trim()));
    }
    let mut bytes = out.stdout;
    if out.truncated {
        // The byte cap can split a trailing UTF-8 sequence; drop the partial.
        if let Err(e) = std::str::from_utf8(&bytes) {
            if bytes.len() - e.valid_up_to() < 4 {
                bytes.truncate(e.valid_up_to());
            }
        }
    }
    Ok(GitFileDiff {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: out.truncated,
    })
}

/// Join `rel` onto `root`, refusing absolute paths and any `..` step — and
/// then confirm against the REAL filesystem: lexical checks can't see
/// symlinks, and a workspace entry (`docs/secret-link → ~/.ssh/config`) must
/// not carry a read or write outside the project. The returned path is
/// canonical, so every operation acts on the file the check approved.
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
    let canon_root = fs::canonicalize(root_path)
        .map_err(|e| format!("not a workspace directory: {root} ({e})"))?;
    let canon = canonicalize_allowing_missing_tail(&resolved)
        .ok_or_else(|| format!("could not resolve {rel}"))?;
    if !canon.starts_with(&canon_root) {
        return Err(format!("path escapes the workspace: {rel}"));
    }
    Ok(canon)
}

/// Canonicalize a path whose tail may not exist yet (a file being created):
/// the deepest existing ancestor is resolved for real — following, and
/// thereby exposing, any symlink — and the uncreated remainder (already
/// lexically validated by the caller) is re-joined. `None` only when nothing
/// on the path exists at all.
fn canonicalize_allowing_missing_tail(path: &Path) -> Option<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs::canonicalize(&existing) {
            Ok(canon) => {
                let mut out = canon;
                for seg in tail.iter().rev() {
                    out.push(seg);
                }
                return Some(out);
            }
            Err(_) => {
                tail.push(existing.file_name()?.to_os_string());
                existing = existing.parent()?.to_path_buf();
            }
        }
    }
}

/// The canonical absolute path of a workspace file, for the webview's asset
/// protocol (markdown-preview images and similar). Same boundary as every
/// other command: a symlink pointing outside the workspace is refused here,
/// BEFORE the path ever reaches `convertFileSrc` — the asset protocol itself
/// would happily follow it.
#[tauri::command]
pub(crate) fn fs_asset_path(root: String, path: String) -> Result<String, String> {
    let file = resolve(&root, &path)?;
    if !file.is_file() {
        return Err(format!("not a file: {path}"));
    }
    Ok(file.display().to_string())
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

    /// Lexical checks can't see symlinks: a workspace entry linking outside
    /// the project must be refused for reads, writes, diffs, and asset
    /// resolution alike — the boundary is the CANONICAL root.
    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_carry_operations_outside_the_workspace() {
        let outside = std::env::temp_dir().join(format!(
            "oxen-harness-files-outside-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "s3cret").unwrap();

        let dir = workspace("symlink");
        std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("leak.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("leakdir")).unwrap();
        // An INTERNAL symlink is fine — the boundary is the workspace, not
        // "no symlinks".
        std::os::unix::fs::symlink(dir.join("README.md"), dir.join("alias.md")).unwrap();

        let root = dir.display().to_string();
        assert!(fs_read_file(root.clone(), "leak.txt".into()).is_err());
        assert!(fs_write_file(root.clone(), "leak.txt".into(), "overwrite".into()).is_err());
        assert!(fs_read_file(root.clone(), "leakdir/secret.txt".into()).is_err());
        assert!(fs_create_entry(root.clone(), "leakdir/new.txt".into(), false).is_err());
        assert!(fs_asset_path(root.clone(), "leak.txt".into()).is_err());
        assert!(git_diff(root.clone(), "leak.txt".into()).is_err());
        assert_eq!(
            fs_read_file(root, "alias.md".into()).unwrap().content,
            "hello"
        );
        // The outside file was never touched.
        assert_eq!(fs::read_to_string(outside.join("secret.txt")).unwrap(), "s3cret");
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    /// The diff cap applies WHILE streaming — a huge diff comes back cut at
    /// the cap instead of ballooning through memory first.
    #[test]
    fn git_diff_streams_and_caps_a_huge_file() {
        let dir = workspace("gitcap");
        git_init(&dir);
        // ~6 MB of changed lines — three times the cap.
        let big: String = (0..200_000)
            .map(|i| format!("line number {i} padded out a bit\n"))
            .collect();
        fs::write(dir.join("big.txt"), big).unwrap();
        let diff = git_diff(dir.display().to_string(), "big.txt".into()).unwrap();
        assert!(diff.truncated);
        assert!(diff.content.len() <= MAX_READ_BYTES);
        assert!(diff.content.contains("line number 0"));
        fs::remove_dir_all(dir).unwrap();
    }

    /// Viewing a diff must not run repository-configured programs: a repo's
    /// .gitattributes can attach a textconv/external-diff helper, and the
    /// safe flags keep it inert.
    #[test]
    fn git_diff_never_runs_repository_configured_helpers() {
        let dir = workspace("gitconv");
        git_init(&dir);
        let marker = dir.join("pwned");
        fs::write(dir.join(".gitattributes"), "*.md diff=evil\n").unwrap();
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
        run(&[
            "config",
            "diff.evil.textconv",
            &format!("touch {} #", marker.display()),
        ]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "attrs"]);
        fs::write(dir.join("README.md"), "changed").unwrap();
        let diff = git_diff(dir.display().to_string(), "README.md".into()).unwrap();
        assert!(diff.content.contains("-hello"), "{}", diff.content);
        assert!(
            !marker.exists(),
            "viewing a diff executed a repository-configured helper"
        );
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
