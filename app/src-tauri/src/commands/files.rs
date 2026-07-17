//! Workspace files — the Files tree and the Editor pane browse, read, and
//! write files inside a chat's working directory. Every command takes the
//! workspace root (the frontend knows it from the session) plus a
//! workspace-relative path, and refuses anything that would escape the root,
//! so the webview can never reach outside the project it's showing.

use std::fs;
use std::path::{Component, Path, PathBuf};

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

/// Editor read cap. Files beyond this open truncated + read-only; anything
/// that big is a build artifact or a dataset, not something to hand-edit.
const MAX_READ_BYTES: usize = 2_000_000;

/// Join `rel` onto `root`, refusing absolute paths and any `..` step so the
/// result provably stays inside the workspace. Shared with the dataset
/// commands, which take the same root + relative-path pair.
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
            let rel = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
            Some(FileEntry { name, path: rel, is_dir })
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
    let slice = if truncated { &bytes[..MAX_READ_BYTES] } else { &bytes[..] };
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
    Ok(FileBody { content, truncated, size: meta.len() })
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
