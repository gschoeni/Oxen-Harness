//! CLI side of the live-preview dev servers. A terminal can't embed a browser,
//! so the CLI's preview story is: the agent starts servers with
//! `start_dev_server` (same tool as the desktop), the sink remembers the
//! latest status, and `/preview` opens the running app in the user's browser.
//!
//! Process-wide statics are honest here for the same reason as
//! `endpoint::FLEET_SPAWNER`: the CLI runs exactly one session.

use std::sync::{Mutex, OnceLock};

use harness_preview::{DevServerManager, PreviewSink, PreviewStatus};

/// The manager key for the CLI's single session.
pub(crate) const SESSION_KEY: &str = "cli";

static MANAGER: OnceLock<DevServerManager> = OnceLock::new();
static STATUS: Mutex<Option<PreviewStatus>> = Mutex::new(None);

/// The process-wide dev-server manager (created on first use).
pub(crate) fn manager() -> DevServerManager {
    MANAGER.get_or_init(DevServerManager::new).clone()
}

/// Remembers the latest lifecycle snapshot for `/preview`. Nothing is printed
/// here — mid-turn output would garble the composer; the tool result already
/// tells the story.
pub(crate) struct CliPreviewSink;

impl PreviewSink for CliPreviewSink {
    fn status(&self, status: &PreviewStatus) {
        *STATUS.lock().unwrap() = Some(status.clone());
    }
}

/// The latest dev-server status, if the agent ever started one.
pub(crate) fn last_status() -> Option<PreviewStatus> {
    STATUS.lock().unwrap().clone()
}

/// Stop any running dev server (REPL exit) — the manager lives in a static, so
/// nothing drops it for us and an orphaned `npm run dev` would outlive the CLI.
pub(crate) async fn shutdown() {
    if let Some(manager) = MANAGER.get() {
        manager.stop_all().await;
    }
}

/// Open a URL in the user's default browser (best-effort, non-blocking).
pub(crate) fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}
