//! Reload-on-change for dev servers without hot reload.
//!
//! Vite/Next/etc. push updates into the page themselves — for those we do
//! nothing. Everything else (python http.server, a static file server, a
//! backend template app) gets a filesystem watch on the workspace: when the
//! agent (or the user's editor) writes project files, the host is asked to
//! reload the preview, debounced so one edit batch is one reload.

use std::path::Path;
use std::time::Duration;

/// Quiet period after the last relevant change before asking for a reload.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Directory/file names whose changes never warrant a reload: VCS internals,
/// dependency and build output trees (the dev server's own artifacts would
/// otherwise cause reload loops), and our own per-project config dir.
const IGNORED: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".oxen-harness",
    "__pycache__",
    ".venv",
    ".DS_Store",
];

/// Command substrings and package.json dependencies that mark a server as
/// hot-reload-capable (it updates the browser itself; watching would only
/// cause double reloads).
const HMR_COMMANDS: &[&str] = &["vite", "next dev", "astro dev", "remix dev", "nuxt dev"];
const HMR_PACKAGES: &[&str] = &[
    "vite",
    "next",
    "astro",
    "nuxt",
    "@remix-run/dev",
    "react-scripts",
    "webpack-dev-server",
    "@sveltejs/kit",
];

/// Whether the server updates the browser itself (framework HMR / live
/// reload), judged from the start command and the project's package.json.
pub fn hmr_capable(root: &Path, command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    if HMR_COMMANDS.iter().any(|c| command.contains(c)) {
        return true;
    }
    // The package.json signal only applies when the command actually runs a
    // package script (`npm run dev`, `pnpm dev`, `npx …`). A `python3 -m
    // http.server` in a repo that happens to depend on vite is NOT
    // hot-reloading — believing so would rob it of its file-watch reload.
    if !runs_package_script(&command) {
        return false;
    }
    let Ok(raw) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    ["dependencies", "devDependencies"].iter().any(|section| {
        pkg[section]
            .as_object()
            .is_some_and(|deps| HMR_PACKAGES.iter().any(|p| deps.contains_key(*p)))
    })
}

/// Whether `command` invokes a Node package manager / script runner, the only
/// case where package.json's dependencies describe what's actually running.
fn runs_package_script(command: &str) -> bool {
    ["npm ", "pnpm ", "yarn", "npx ", "bun ", "node "]
        .iter()
        .any(|runner| command.contains(runner))
}

/// Whether a changed path is project content worth reloading for.
fn relevant(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    !rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|name| IGNORED.contains(&name))
    })
}

/// How the polling fallback paces its scans. Coarser than FSEvents, but a
/// reload that arrives a second late beats one that never arrives.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How long the canary waits for the native watcher to prove it delivers
/// before the polling fallback takes over (total, spread over retries).
const VERIFY_WINDOW: Duration = Duration::from_secs(3);
const VERIFY_STEPS: u32 = 6;

/// A live workspace watch. Holds whichever backend won — the platform's
/// native watcher, or the polling fallback that takes over when the native
/// one is silently broken (seen in the wild: macOS FSEvents delivering
/// nothing at all under newer macOS releases). Keep it alive as long as
/// reloads are wanted; dropping it ends the watch and its task.
pub(crate) struct WorkspaceWatcher {
    // Never read here — held so its Drop ends the watch; the canary task
    // holds a Weak to it and swaps in the polling backend when needed.
    #[allow(dead_code)]
    backend: std::sync::Arc<std::sync::Mutex<Backend>>,
}

enum Backend {
    Native(#[allow(dead_code)] notify::RecommendedWatcher),
    Poll(#[allow(dead_code)] notify::PollWatcher),
}

/// Watch `root` recursively and call `on_change` (debounced) for relevant
/// changes.
///
/// Trust, but verify: the native watcher is started first, then a canary file
/// is written under the workspace's `.oxen-harness/` dir. If no event at all
/// arrives within the verify window, the native backend is presumed dead and
/// a polling watcher silently takes its place — the reload keeps working
/// either way, it just costs a periodic scan.
pub(crate) fn spawn(
    root: &Path,
    on_change: impl Fn() + Send + 'static,
) -> notify::Result<WorkspaceWatcher> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    // macOS FSEvents reports canonical paths (`/private/tmp/…`), so a
    // symlinked root would fail to strip — and then an absolute path with a
    // component named `build`/`dist` would filter out the whole workspace.
    let filter_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // Set by the event handler on ANY delivery — real edits and the canary
    // alike prove the backend is alive.
    let heard = Arc::new(AtomicBool::new(false));

    let handler = {
        let heard = heard.clone();
        move |event: notify::Result<notify::Event>| {
            use notify::event::{EventKind, ModifyKind};
            let Ok(event) = event else { return };
            heard.store(true, Ordering::SeqCst);
            let meaningful = match event.kind {
                EventKind::Create(_) | EventKind::Remove(_) => true,
                // A metadata change on a DIRECTORY is noise — and worse:
                // writing inside an IGNORED tree (dist/, node_modules/) bumps
                // its parent directory's mtime, which the polling backend
                // reports as a WriteTime change on a path that passes the
                // relevance filter — the exact reload loop the ignore list
                // exists to prevent. On a FILE the same kind is how the
                // polling backend reports an ordinary edit, so it must count.
                EventKind::Modify(ModifyKind::Metadata(_)) => {
                    event.paths.iter().any(|p| p.is_file())
                }
                EventKind::Modify(_) => true,
                _ => false,
            };
            if !meaningful {
                return;
            }
            // The canary lives under `.oxen-harness/`, which `relevant`
            // already ignores — proving liveness never triggers a reload.
            if event.paths.iter().any(|p| relevant(&filter_root, p)) {
                let _ = tx.send(());
            }
        }
    };

    let fallback_handler = handler.clone();
    let mut watcher = notify::recommended_watcher(handler)?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    let backend = Arc::new(Mutex::new(Backend::Native(watcher)));

    // Debounce: after any change, absorb further ones until DEBOUNCE of quiet,
    // then reload once. The task ends when every sender (the watcher) drops.
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            loop {
                match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
                    Ok(Some(())) => continue, // still changing — keep absorbing
                    Ok(None) => return,       // watcher dropped mid-burst
                    Err(_) => break,          // quiet period reached
                }
            }
            on_change();
        }
    });

    // The canary: poke the watched tree until the backend speaks, and swap in
    // the polling fallback if it never does. Holds only a weak handle — if
    // the watch is dropped mid-verify, there is nothing left to verify.
    let weak = Arc::downgrade(&backend);
    let root = root.to_path_buf();
    tokio::spawn(async move {
        let canary_dir = root.join(".oxen-harness");
        let canary = canary_dir.join(".watch-canary");
        let mut alive = false;
        for step in 0..VERIFY_STEPS {
            let _ = std::fs::create_dir_all(&canary_dir);
            let _ = std::fs::write(&canary, step.to_string());
            tokio::time::sleep(VERIFY_WINDOW / VERIFY_STEPS).await;
            if heard.load(Ordering::SeqCst) {
                alive = true;
                break;
            }
            if weak.strong_count() == 0 {
                break; // the watch was dropped — stop poking the workspace
            }
        }
        let _ = std::fs::remove_file(&canary);
        let Some(backend) = weak.upgrade() else { return };
        if alive {
            return;
        }
        let poll = notify::PollWatcher::new(
            fallback_handler,
            notify::Config::default().with_poll_interval(POLL_INTERVAL),
        )
        .and_then(|mut poll| poll.watch(&root, RecursiveMode::Recursive).map(|()| poll));
        match poll {
            Ok(poll) => {
                tracing::warn!(
                    "native file watcher delivered nothing for {}; falling back to polling",
                    root.display()
                );
                *backend.lock().unwrap() = Backend::Poll(poll);
            }
            Err(e) => tracing::warn!("preview reload poll fallback failed: {e}"),
        }
    });

    Ok(WorkspaceWatcher { backend })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn hmr_detection_from_command_and_package_json() {
        let dir = tempfile::tempdir().unwrap();
        assert!(hmr_capable(dir.path(), "npm run vite"));
        assert!(hmr_capable(dir.path(), "next dev"));
        assert!(!hmr_capable(dir.path(), "python3 -m http.server"));

        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"^6.0.0"}}"#,
        )
        .unwrap();
        // `npm run dev` says nothing by itself; package.json breaks the tie.
        assert!(hmr_capable(dir.path(), "npm run dev"));
        // …but a non-Node server in the same repo is NOT hot-reloading, and
        // must keep its file-watch reload.
        assert!(!hmr_capable(dir.path(), "python3 -m http.server \"$PORT\""));
    }

    #[test]
    fn ignores_dependency_and_build_trees() {
        let root = Path::new("/proj");
        assert!(relevant(root, Path::new("/proj/src/App.tsx")));
        assert!(relevant(root, Path::new("/proj/index.html")));
        assert!(!relevant(
            root,
            Path::new("/proj/node_modules/react/index.js")
        ));
        assert!(!relevant(root, Path::new("/proj/.git/HEAD")));
        assert!(!relevant(root, Path::new("/proj/dist/bundle.js")));
        assert!(!relevant(
            root,
            Path::new("/proj/.oxen-harness/preview.json")
        ));
    }

    /// The long waits here are not superstition: the watch may be riding the
    /// polling fallback (macOS FSEvents delivers nothing on some releases —
    /// the reason the fallback exists), so every settle must outlast
    /// POLL_INTERVAL + DEBOUNCE, not just the debounce.
    const SETTLE: Duration = Duration::from_millis(1_800);

    #[tokio::test]
    async fn one_edit_batch_becomes_one_reload() {
        let dir = tempfile::tempdir().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let watcher = spawn(dir.path(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        // Warm up until SOME backend delivers — the native watcher right
        // away, or the canary-verified polling fallback a few seconds in.
        for round in 0..30 {
            std::fs::write(dir.path().join("warmup.html"), round.to_string()).unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
            if hits.load(Ordering::SeqCst) > 0 {
                break;
            }
        }
        assert!(hits.load(Ordering::SeqCst) > 0, "no watch backend ever delivered an event");

        // Let every pending report drain, then phase-align on the backend:
        // one probe write, and the moment its reload lands we know a scan
        // (or event flush) just finished — the next one is a full interval
        // away, so a burst written NOW can't straddle a scan boundary.
        tokio::time::sleep(SETTLE).await;
        let probe_base = hits.load(Ordering::SeqCst);
        std::fs::write(dir.path().join("probe.html"), "align").unwrap();
        for _ in 0..40 {
            if hits.load(Ordering::SeqCst) > probe_base {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let before = hits.load(Ordering::SeqCst);
        assert!(before > probe_base, "probe write was never reported");

        // A burst of writes (an agent edit batch)…
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.html")), "<p>hi</p>").unwrap();
        }
        // …must produce exactly one (debounced) reload.
        for _ in 0..40 {
            if hits.load(Ordering::SeqCst) > before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::sleep(SETTLE).await; // absorb any would-be stragglers
        assert_eq!(hits.load(Ordering::SeqCst), before + 1);

        // Changes under an ignored tree must not reload.
        let modules = dir.path().join("node_modules");
        std::fs::create_dir(&modules).unwrap();
        std::fs::write(modules.join("dep.js"), "x").unwrap();
        tokio::time::sleep(SETTLE).await;
        assert_eq!(hits.load(Ordering::SeqCst), before + 1);

        drop(watcher);
    }
}
