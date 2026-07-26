//! Open a project directory handed over from the command line.
//!
//! `oxen-harness ui [dir]` launches the desktop app with the directory as a
//! plain positional argument. On a cold start [`crate::run`] reads it from
//! `std::env::args` and makes it the active project before any state is
//! built. When the app is already running, the single-instance plugin
//! forwards the second launch's argv (and its cwd) here instead: the window
//! is focused and a `project://open` event tells the UI to enter the project.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager};

use crate::commands::project::remember_project;
use crate::events::ProjectOpenPayload;
use crate::state::AppState;

/// The project directory in a launch argv: the first non-flag argument after
/// the program name that names a directory — resolved against `base` (the
/// invoking process's cwd, so relative paths mean what the caller meant) and
/// canonicalized so it matches the key the projects store uses.
pub(crate) fn dir_from_args(args: impl IntoIterator<Item = String>, base: &Path) -> Option<String> {
    args.into_iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .find_map(|arg| {
            let path = base.join(&arg);
            path.is_dir()
                .then(|| path.canonicalize().unwrap_or(path).display().to_string())
        })
}

/// The single-instance callback: a second `oxen-harness ui <dir>` (or plain
/// second launch) landed while this instance owns the app. Focus the window;
/// if the argv carries a directory, make it the active project and hand it to
/// the UI.
pub(crate) fn open_from_second_instance(app: &AppHandle, argv: &[String], cwd: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let Some(dir) = dir_from_args(argv.iter().cloned(), Path::new(cwd)) else {
        return;
    };
    let _ = remember_project(&dir);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        *state.active_project.lock().await = PathBuf::from(&dir);
        let _ = app.emit("project://open", ProjectOpenPayload { path: dir });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn first_existing_directory_wins_and_flags_are_skipped() {
        let tmp =
            std::env::temp_dir().join(format!("oxen-harness-cli-open-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir(&tmp).unwrap();
        let tmp_str = tmp.display().to_string();

        let found = dir_from_args(
            args(&["app-binary", "--flag", "/definitely/not/a/dir", &tmp_str]),
            Path::new("/"),
        );
        assert_eq!(
            found,
            Some(tmp.canonicalize().unwrap().display().to_string())
        );
        std::fs::remove_dir(&tmp).unwrap();
    }

    #[test]
    fn relative_paths_resolve_against_the_callers_cwd() {
        let tmp =
            std::env::temp_dir().join(format!("oxen-harness-cli-open-rel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("inner")).unwrap();

        let found = dir_from_args(args(&["app-binary", "inner"]), &tmp);
        assert_eq!(
            found,
            Some(
                tmp.join("inner")
                    .canonicalize()
                    .unwrap()
                    .display()
                    .to_string()
            )
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn no_directory_in_argv_means_none() {
        assert_eq!(dir_from_args(args(&["app-binary"]), Path::new("/")), None);
        assert_eq!(
            dir_from_args(args(&["app-binary", "--flag", "/nope"]), Path::new("/")),
            None
        );
        // The program name itself must never be mistaken for the directory.
        assert_eq!(dir_from_args(args(&["/"]), Path::new("/")), None);
    }
}
