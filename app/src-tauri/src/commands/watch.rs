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

/// One debounce window's worth of changed paths, with the cap enforced as
/// they arrive. Once over the cap the set is dropped outright — the batch is
/// already "everything", so holding (or growing) the list would only cost
/// memory during exactly the storms the cap exists for.
struct Batch {
    paths: BTreeSet<String>,
    overflowed: bool,
}

impl Batch {
    fn new() -> Self {
        Self {
            paths: BTreeSet::new(),
            overflowed: false,
        }
    }

    /// Fold in more paths, stopping the moment the distinct count exceeds
    /// the cap. Distinct, not raw: FS backends re-report the same file many
    /// times in a burst, and 600 events on one file are not "too much to
    /// enumerate".
    fn absorb(&mut self, more: impl IntoIterator<Item = String>) {
        if self.overflowed {
            return;
        }
        for path in more {
            self.paths.insert(path);
            if self.paths.len() > MAX_PATHS {
                self.overflowed = true;
                self.paths = BTreeSet::new();
                return;
            }
        }
    }

    /// The payload's path list: empty means "refresh everything".
    fn into_paths(self) -> Vec<String> {
        if self.overflowed {
            Vec::new()
        } else {
            self.paths.into_iter().collect()
        }
    }
}

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
/// under an ignored tree: `.git` (index locks churn constantly and the tree
/// hides it anyway) plus the dependency/build directories the preview's
/// reload watcher already ignores (`node_modules`, `target`, `dist`, …). A
/// `cargo build` writes tens of thousands of files under `target/` and an
/// `npm install` even more under `node_modules/`; without this every one of
/// them would be relativized, deduped, and — as a "too much changed" batch —
/// make the UI re-list every open directory and re-run `git status`, many
/// times over during the build. The trade: a `dist/` the user has expanded
/// in the tree won't refresh on its own until the next batch or manual
/// refresh. Runs inside the notify callback, so it bails before allocating
/// the segment list.
/// Tries the canonical root too: macOS FSEvents reports `/private/tmp/...`
/// for a workspace opened as `/tmp/...`.
fn workspace_rel(root: &Path, canonical: &Path, abs: &Path) -> Option<String> {
    let rel = abs
        .strip_prefix(root)
        .or_else(|_| abs.strip_prefix(canonical))
        .ok()?;
    if rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(harness_preview::is_ignored_name)
    }) {
        return None;
    }
    let mut parts = Vec::new();
    for part in rel.components() {
        parts.push(part.as_os_str().to_string_lossy().into_owned());
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
    let mut watchers = state.0.lock().map_err(|_| {
        "file watchers are unavailable after a failed operation; restart the application"
    })?;
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
            let mut batch = Batch::new();
            batch.absorb(first);
            let deadline = Instant::now() + DEBOUNCE;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    // Past the cap `absorb` is a no-op: keep draining the
                    // channel so the window still closes on time.
                    Ok(more) => batch.absorb(more),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let _ = app.emit(
                "fs://changed",
                FsChangedPayload {
                    root: emit_root.clone(),
                    paths: batch.into_paths(),
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
    state
        .0
        .lock()
        .map_err(|_| {
            "file watchers are unavailable after a failed operation; restart the application"
        })?
        .remove(&root);
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

    /// Build and dependency trees are dropped at any depth (a `cargo build`
    /// or `npm install` must not become thousands of UI refreshes), but a
    /// FILE that merely shares the name is still project content.
    #[test]
    fn drops_build_and_dependency_trees() {
        let root = Path::new("/ws");
        let canonical = Path::new("/private/ws");
        for ignored in [
            "/ws/target/debug/deps/libfoo.rlib",
            "/ws/node_modules/react/index.js",
            "/ws/packages/web/node_modules/x/y.js",
            "/ws/dist/bundle.js",
            "/private/ws/target/x",
            "/ws/.DS_Store",
        ] {
            assert_eq!(
                workspace_rel(root, canonical, Path::new(ignored)),
                None,
                "{ignored} should be ignored"
            );
        }
        assert_eq!(
            workspace_rel(root, canonical, Path::new("/ws/src/target.rs")),
            Some("src/target.rs".into())
        );
        assert_eq!(
            workspace_rel(root, canonical, Path::new("/ws/docs/build-notes.md")),
            Some("docs/build-notes.md".into())
        );
    }

    /// The batch never holds more than the cap: one path over and it flips
    /// to "everything changed" (an empty list) and stops accumulating, no
    /// matter how large the incoming chunks are.
    #[test]
    fn batch_caps_distinct_paths_and_overflows_to_empty() {
        let mut batch = Batch::new();
        batch.absorb((0..MAX_PATHS).map(|i| format!("f{i}")));
        assert!(!batch.overflowed);
        assert_eq!(batch.paths.len(), MAX_PATHS);
        // Re-reports of the same files don't count toward the cap.
        batch.absorb((0..MAX_PATHS).map(|i| format!("f{i}")));
        assert!(!batch.overflowed);

        // One genuinely new path tips it over — in the middle of a big
        // chunk, which must not be retained past the cap.
        batch.absorb((MAX_PATHS..MAX_PATHS + 10_000).map(|i| format!("f{i}")));
        assert!(batch.overflowed);
        assert!(batch.paths.is_empty());
        batch.absorb(["late".to_string()]);
        assert!(batch.paths.is_empty());
        assert!(batch.into_paths().is_empty());

        let mut small = Batch::new();
        small.absorb(["b".to_string(), "a".to_string(), "b".to_string()]);
        assert_eq!(small.into_paths(), vec!["a".to_string(), "b".to_string()]);
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
