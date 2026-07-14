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

/// Watch `root` recursively and call `on_change` (debounced) for relevant
/// changes. Returns the watcher — keep it alive as long as reloads are wanted;
/// dropping it ends the watch and its task.
pub(crate) fn spawn(
    root: &Path,
    on_change: impl Fn() + Send + 'static,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    // macOS FSEvents reports canonical paths (`/private/tmp/…`), so a
    // symlinked root would fail to strip — and then an absolute path with a
    // component named `build`/`dist` would filter out the whole workspace.
    let filter_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if !event.kind.is_create() && !event.kind.is_modify() && !event.kind.is_remove() {
            return;
        }
        if event.paths.iter().any(|p| relevant(&filter_root, p)) {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    // Debounce: after any change, absorb further ones until DEBOUNCE of quiet,
    // then reload once. The task ends when the watcher (the sender) drops.
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

    Ok(watcher)
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

    #[tokio::test]
    async fn one_edit_batch_becomes_one_reload() {
        let dir = tempfile::tempdir().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let watcher = spawn(dir.path(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        // A burst of writes (an agent edit batch)…
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.html")), "<p>hi</p>").unwrap();
        }
        // …must produce exactly one (debounced) reload.
        for _ in 0..100 {
            if hits.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Changes under an ignored tree must not reload.
        let modules = dir.path().join("node_modules");
        std::fs::create_dir(&modules).unwrap();
        std::fs::write(modules.join("dep.js"), "x").unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        drop(watcher);
    }
}
