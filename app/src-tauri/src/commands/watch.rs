//! Workspace filesystem watching — the Files tree and the Editor pane refresh
//! when *other* processes (the agent's shell, builds, git, another editor)
//! touch files on disk. One native recursive watcher per workspace root
//! (FSEvents on macOS, inotify on Linux, ReadDirectoryChangesW on Windows);
//! raw events are debounced into batches and emitted to the webview as one
//! `fs://changed` event carrying workspace-relative paths.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// FS events arrive in bursts (a build, `npm install`, a git checkout);
/// collect for this long after the first one, then emit a single batch.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Past this many distinct paths in one batch the event degrades to
/// `paths: []` — "a lot changed, refresh whatever you're showing" — instead
/// of shipping a giant list across the IPC boundary.
const MAX_PATHS: usize = 512;

/// Live watchers keyed by workspace root. Dropping a watcher (unwatch or
/// app exit) closes its channel, which ends its emitter thread.
#[derive(Default)]
pub(crate) struct FsWatchState(Mutex<HashMap<String, RecommendedWatcher>>);

/// The `fs://changed` payload. Empty `paths` means "too much changed to
/// enumerate — refresh everything you have loaded for this root".
#[derive(Clone, Serialize)]
struct FsChangedPayload {
    root: String,
    paths: Vec<String>,
}

/// Only mutations matter; access/metadata-only chatter would wake the UI for
/// nothing. `Any` stays in because backends use it for coalesced events.
fn is_mutation(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

/// Relativize an event path against the workspace root, dropping anything
/// under `.git` (index locks churn constantly and the tree hides it anyway).
/// Tries the canonical root too: macOS FSEvents reports `/private/tmp/...`
/// for a workspace opened as `/tmp/...`.
fn workspace_rel(root: &Path, canonical: &Path, abs: &Path) -> Option<String> {
    let rel = abs
        .strip_prefix(root)
        .or_else(|_| abs.strip_prefix(canonical))
        .ok()?;
    let mut parts = Vec::new();
    for part in rel.components() {
        let seg = part.as_os_str().to_string_lossy();
        if seg == ".git" {
            return None;
        }
        parts.push(seg.into_owned());
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Start watching a workspace root (idempotent — a second call for the same
/// root is a no-op, so every interested view can just ask).
#[tauri::command]
pub(crate) fn fs_watch(
    app: AppHandle,
    state: State<'_, FsWatchState>,
    root: String,
) -> Result<(), String> {
    let mut watchers = state.0.lock().unwrap();
    if watchers.contains_key(&root) {
        return Ok(());
    }
    let root_path = PathBuf::from(&root);
    if !root_path.is_absolute() || !root_path.is_dir() {
        return Err(format!("not a workspace directory: {root}"));
    }
    let canonical = std::fs::canonicalize(&root_path).unwrap_or_else(|_| root_path.clone());

    let (tx, rx) = mpsc::channel::<Vec<String>>();
    let cb_root = root_path.clone();
    let cb_canonical = canonical.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !is_mutation(&event.kind) {
            return;
        }
        let rels: Vec<String> = event
            .paths
            .iter()
            .filter_map(|p| workspace_rel(&cb_root, &cb_canonical, p))
            .collect();
        if !rels.is_empty() {
            let _ = tx.send(rels);
        }
    })
    .map_err(|e| format!("could not create watcher: {e}"))?;
    watcher
        .watch(&root_path, RecursiveMode::Recursive)
        .map_err(|e| format!("could not watch {root}: {e}"))?;

    // The emitter thread: soak up a burst, then one event to the webview.
    // It lives exactly as long as the watcher — dropping the watcher drops
    // the callback (the only sender), recv() errors, and the loop ends.
    let emit_root = root.clone();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut paths: BTreeSet<String> = first.into_iter().collect();
            let deadline = Instant::now() + DEBOUNCE;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    Ok(more) => {
                        // Past the cap the batch is already "everything";
                        // keep draining but stop accumulating.
                        if paths.len() <= MAX_PATHS {
                            paths.extend(more);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let list = if paths.len() > MAX_PATHS {
                Vec::new()
            } else {
                paths.into_iter().collect()
            };
            let _ = app.emit(
                "fs://changed",
                FsChangedPayload {
                    root: emit_root.clone(),
                    paths: list,
                },
            );
        }
    });

    watchers.insert(root, watcher);
    Ok(())
}

/// Stop watching a workspace root (no-op if it wasn't watched).
#[tauri::command]
pub(crate) fn fs_unwatch(state: State<'_, FsWatchState>, root: String) -> Result<(), String> {
    state.0.lock().unwrap().remove(&root);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relativizes_and_filters_git_paths() {
        let root = Path::new("/ws");
        let canonical = Path::new("/private/ws");
        assert_eq!(
            workspace_rel(root, canonical, Path::new("/ws/src/main.rs")),
            Some("src/main.rs".into())
        );
        // The canonical alias resolves too (macOS /tmp → /private/tmp).
        assert_eq!(
            workspace_rel(root, canonical, Path::new("/private/ws/a.md")),
            Some("a.md".into())
        );
        assert_eq!(
            workspace_rel(root, canonical, Path::new("/ws/.git/index.lock")),
            None
        );
        assert_eq!(
            workspace_rel(root, canonical, Path::new("/elsewhere/x")),
            None
        );
        // An event on the root itself carries no path to refresh.
        assert_eq!(workspace_rel(root, canonical, Path::new("/ws")), None);
    }

    #[test]
    fn only_mutations_count() {
        assert!(is_mutation(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_mutation(&EventKind::Any));
        assert!(!is_mutation(&EventKind::Access(
            notify::event::AccessKind::Read
        )));
    }
}
