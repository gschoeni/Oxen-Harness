//! App updates: which version is running, whether GitHub has a newer desktop
//! release, and installing it in place (macOS) before relaunching. Other
//! platforms install from the release page; the status says which applies.

use harness_update::{Component, UpdateStatus, Updater, CHECK_MAX_AGE};
use std::time::Duration;

fn current_version(app: &tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Compare the running app with the newest `app-v*` release. The launch
/// check passes `force: false` and reuses an answer up to an hour old; the
/// Settings "Check now" button forces a fresh one.
#[tauri::command]
pub(crate) async fn check_for_update(
    app: tauri::AppHandle,
    force: bool,
) -> Result<UpdateStatus, String> {
    let cache = harness_config::paths::base_dir()
        .map(|dir| harness_update::cache_path(&dir, Component::App))
        .map_err(|e| e.to_string())?;
    let max_age = if force { Duration::ZERO } else { CHECK_MAX_AGE };
    Updater::default()
        .check(
            Component::App,
            &current_version(&app),
            Some(&cache),
            max_age,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Download the newest release, verify it, swap the running `.app` bundle
/// for it, and relaunch. Only returns on failure (or when already current).
#[tauri::command]
pub(crate) async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = Updater::default();
    let release = updater
        .latest(Component::App)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no desktop release is published yet")?;
    let current = harness_update::Version::parse(&current_version(&app))
        .ok_or("this build has no release version")?;
    if release.version <= current {
        return Err(format!("already on the latest version (v{current})"));
    }
    let exe = std::env::current_exe().map_err(|e| format!("finding the running app: {e}"))?;
    let bundle = harness_update::app_bundle_of(&exe).ok_or_else(|| {
        format!(
            "this copy isn't an installed app bundle ({}): download v{} from {}",
            exe.display(),
            release.version,
            release.page_url
        )
    })?;
    let archive = updater
        .download(Component::App, &release)
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        harness_update::install_app_bundle(&archive, &bundle)
    })
    .await
    .map_err(|e| format!("the install did not finish: {e}"))?
    .map_err(|e| e.to_string())?;
    app.restart()
}
