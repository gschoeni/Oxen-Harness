//! The live-preview pane: a native child webview showing the session's running
//! dev server, positioned over the React layout's placeholder.
//!
//! The Rust side owns the webview (creation, bounds, show/hide, navigation);
//! the frontend drives it through the `preview_*` commands as its placeholder
//! mounts, resizes, and unmounts. One child webview per session, labeled
//! `preview-<session>`, so switching chats swaps which one is visible. The
//! webview loads external (localhost) content and therefore gets no Tauri IPC.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use harness_preview::console::{init_script, ConsoleLine};
use harness_preview::{ConsoleBridge, PreviewLens, PreviewSink, PreviewStatus};

use crate::events::{PreviewConsolePayload, PreviewStatusPayload};
use crate::state::AppState;

/// Child-webview label prefix; the suffix is the session id (a UUID, which is
/// valid label charset).
const LABEL_PREFIX: &str = "preview-";

pub(crate) fn label_for(session: &str) -> String {
    format!("{LABEL_PREFIX}{session}")
}

/// Bridges dev-server lifecycle changes to the UI (`preview://status`), one
/// sink per session. On stop/error it also hides that session's webview —
/// there is nothing live to show, and a dead pane must never linger over the
/// chat.
pub(crate) struct TauriPreviewSink {
    pub(crate) app: AppHandle,
    pub(crate) session: String,
}

impl PreviewSink for TauriPreviewSink {
    fn status(&self, status: &PreviewStatus) {
        if !matches!(
            status.phase,
            harness_preview::PreviewPhase::Starting | harness_preview::PreviewPhase::Ready
        ) {
            if let Some(webview) = self.app.webviews().get(&label_for(&self.session)) {
                let _ = webview.hide();
                mark_hidden(&self.session);
            }
        }
        // A fresh (re)start begins with a clean console slate, and the page
        // sitting in the webview belongs to the *old* process — force the next
        // attach to re-navigate even when the URL is byte-identical (a pinned
        // port), or the user would keep staring at a dead render.
        if status.phase == harness_preview::PreviewPhase::Ready {
            if let Some(bridge) = self.app.state::<AppState>().console_bridge.get() {
                bridge.clear(&self.session);
            }
            invalidate(&self.session);
        }
        let _ = self.app.emit(
            "preview://status",
            PreviewStatusPayload {
                session: self.session.clone(),
                status: status.clone(),
            },
        );
    }

    fn reload_needed(&self) {
        reload(&self.app, &self.session);
    }
}

/// How the agent sees the preview: native WKWebView snapshots for
/// `preview_screenshot` and the console bridge's buffer for `preview_console`.
pub(crate) struct TauriPreviewLens {
    pub(crate) app: AppHandle,
    pub(crate) session: String,
}

#[async_trait]
impl PreviewLens for TauriPreviewLens {
    async fn screenshot(&self) -> Result<Vec<u8>, String> {
        #[cfg(target_os = "macos")]
        {
            // A hidden webview (the user is looking at another chat, or an
            // overlay is up) has its rendering throttled by WebKit, so a
            // snapshot can show a pre-update frame. Verifying against a stale
            // render is worse than not verifying — say so instead.
            if !is_shown(&self.session) {
                return Err("the preview isn't visible right now (the user is on \
                            another screen or chat), so a screenshot would show a \
                            stale frame — verify with dev_server_logs and \
                            preview_console instead"
                    .to_string());
            }
            let webview = self
                .app
                .webviews()
                .get(&label_for(&self.session))
                .cloned()
                .ok_or_else(|| {
                    "the preview isn't open in the app window (the user may have \
                     closed it or be looking at another chat) — ask them to open \
                     the Preview panel, or verify with dev_server_logs instead"
                        .to_string()
                })?;
            crate::snapshot::take_png(webview).await
        }
        #[cfg(not(target_os = "macos"))]
        Err("preview screenshots aren't supported on this platform yet — verify with \
             dev_server_logs and preview_console instead"
            .to_string())
    }

    fn console_tail(&self, n: usize) -> Vec<ConsoleLine> {
        self.app
            .state::<AppState>()
            .console_bridge
            .get()
            .map(|bridge| bridge.tail(&self.session, n))
            .unwrap_or_default()
    }
}

/// The app-wide console bridge, started on first use. Error-level lines are
/// forwarded to the UI (`preview://console`) for the "Fix it" banner.
pub(crate) async fn console_bridge(app: &AppHandle) -> Result<Arc<ConsoleBridge>, String> {
    let state = app.state::<AppState>();
    let emitter = app.clone();
    state
        .console_bridge
        .get_or_try_init(|| async move {
            ConsoleBridge::start(Arc::new(move |session, text| {
                // `None` = the page reloaded, so whatever it complained about
                // is history: an empty text clears the UI's banner.
                let _ = emitter.emit(
                    "preview://console",
                    PreviewConsolePayload {
                        session: session.to_string(),
                        text: text.unwrap_or_default().to_string(),
                    },
                );
            }))
            .await
            .map_err(|e| format!("console bridge: {e}"))
        })
        .await
        .cloned()
}

/// The placeholder rectangle, in CSS (logical) pixels relative to the window's
/// content area — exactly what `getBoundingClientRect` yields in the main
/// webview, which fills the window.
#[derive(serde::Deserialize)]
pub(crate) struct Bounds {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// The URL each session's preview was last pointed at by [`attach`].
///
/// The source of truth for "has the server moved?", deliberately *not*
/// `Webview::url()` (the live page): the app may navigate itself to a
/// sub-page or to the other loopback spelling (`localhost` vs `127.0.0.1`,
/// which are distinct origins), and comparing against that would re-navigate
/// on every layout pass — a reload loop while the user drags the splitter.
static NAVIGATED: StdMutex<Option<HashMap<String, String>>> = StdMutex::new(None);

/// Bumped by every [`detach_all`]. A slow first attach (creating the webview
/// takes a beat) that finishes *after* an overlay opened must not `show()` the
/// native view over it — native webviews paint above all DOM.
static DETACH_EPOCH: AtomicU64 = AtomicU64::new(0);

/// The session whose preview is currently *shown*, if any. WebKit throttles
/// rendering in hidden webviews, so `preview_screenshot` consults this rather
/// than photographing a frame that may predate the change it's verifying.
static SHOWN: StdMutex<Option<String>> = StdMutex::new(None);

/// Whether `session`'s preview is the one on screen right now. Only the
/// macOS screenshot path consults this (WebKit throttles hidden webviews);
/// the cfg keeps it from being dead code on other targets.
#[cfg(target_os = "macos")]
pub(crate) fn is_shown(session: &str) -> bool {
    SHOWN.lock().unwrap().as_deref() == Some(session)
}

/// Note that `session`'s preview is no longer on screen.
fn mark_hidden(session: &str) {
    let mut shown = SHOWN.lock().unwrap();
    if shown.as_deref() == Some(session) {
        *shown = None;
    }
}

fn navigated_url(session: &str) -> Option<String> {
    NAVIGATED
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(session).cloned())
}

fn set_navigated_url(session: &str, url: &str) {
    NAVIGATED
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(session.to_string(), url.to_string());
}

fn forget_navigated_url(session: &str) {
    if let Some(map) = NAVIGATED.lock().unwrap().as_mut() {
        map.remove(session);
    }
}

/// Show `session`'s preview webview at `bounds`, creating it (pointed at
/// `url`, with the console-bridge script injected) on first use, navigating it
/// when the server moved (a restart on a new port) or was restarted on the
/// same URL (the page in the view is dead), and hiding every other session's
/// preview.
pub(crate) fn attach(
    app: &AppHandle,
    session: &str,
    url: &str,
    bounds: &Bounds,
    console_port: Option<u16>,
) -> Result<(), String> {
    let label = label_for(session);
    let target: Url = url.parse().map_err(|e| format!("bad preview url: {e}"))?;
    let epoch = DETACH_EPOCH.load(Ordering::SeqCst);

    for (other_label, webview) in app.webviews() {
        if other_label.starts_with(LABEL_PREFIX) && other_label != label {
            let _ = webview.hide();
        }
    }

    let position = LogicalPosition::new(bounds.x, bounds.y);
    let size = LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0));
    // A detach raced us (an overlay opened while we were working) — position
    // the view but leave it hidden; the next attach will show it.
    let show = |webview: &tauri::webview::Webview<tauri::Wry>| {
        if DETACH_EPOCH.load(Ordering::SeqCst) == epoch {
            let _ = webview.show();
            *SHOWN.lock().unwrap() = Some(session.to_string());
        }
    };

    if let Some(webview) = app.webviews().get(&label).cloned() {
        // Re-navigate only when *we* last pointed it somewhere else (a restart
        // on a new port), or when the server restarted on the same URL, which
        // `preview_restart`/the tool signal by forgetting the tracked URL.
        if navigated_url(session).as_deref() != Some(url) {
            webview
                .navigate(target)
                .map_err(|e| format!("navigate preview: {e}"))?;
            set_navigated_url(session, url);
        }
        webview.set_position(position).map_err(|e| e.to_string())?;
        webview.set_size(size).map_err(|e| e.to_string())?;
        show(&webview);
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    let mut builder = WebviewBuilder::new(&label, WebviewUrl::External(target.clone()))
        // The preview is a window onto the user's own dev server, not a
        // browser. Anything else (a link in their app, an ad iframe's popup)
        // opens in the real browser instead — an external page in here would
        // run our console-injection script and could feed crafted "errors"
        // straight to the agent, which reads them automatically when
        // auto-verify is on.
        .on_navigation(move |url: &Url| {
            let allowed = is_loopback(url) || url.scheme() == "about";
            if !allowed {
                let _ = open_external(url.as_str());
            }
            allowed
        });
    if let Some(port) = console_port {
        builder = builder.initialization_script(init_script(port, session));
    }
    window
        .add_child(builder, position, size)
        .map_err(|e| format!("create preview webview: {e}"))?;
    set_navigated_url(session, url);
    // Creation shows the webview; if an overlay opened meanwhile, hide it.
    if DETACH_EPOCH.load(Ordering::SeqCst) != epoch {
        detach_all(app);
    }
    Ok(())
}

/// Whether a URL points at this machine (the user's dev server).
fn is_loopback(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
    )
}

/// Hide every preview webview (tab switched away, overlay opened, pane closed).
pub(crate) fn detach_all(app: &AppHandle) {
    DETACH_EPOCH.fetch_add(1, Ordering::SeqCst);
    *SHOWN.lock().unwrap() = None;
    for (label, webview) in app.webviews() {
        if label.starts_with(LABEL_PREFIX) {
            let _ = webview.hide();
        }
    }
}

/// Reload `session`'s preview (manual refresh button, file-watcher reload).
/// The console buffer resets with the page — stale errors must not linger
/// past the reload that may have fixed them.
pub(crate) fn reload(app: &AppHandle, session: &str) {
    if let Some(bridge) = app.state::<AppState>().console_bridge.get() {
        bridge.clear(session);
    }
    if let Some(webview) = app.webviews().get(&label_for(session)) {
        let _ = webview.eval("location.reload()");
    }
}

/// Forget the URL we last navigated `session`'s preview to, so the next attach
/// re-navigates even if the URL is unchanged. Called when the server restarts:
/// the page currently in the view belongs to a process that no longer exists.
pub(crate) fn invalidate(session: &str) {
    forget_navigated_url(session);
}

/// Destroy `session`'s preview webview entirely (its server is gone).
pub(crate) fn close(app: &AppHandle, session: &str) {
    forget_navigated_url(session);
    mark_hidden(session);
    if let Some(webview) = app.webviews().get(&label_for(session)) {
        let _ = webview.close();
    }
}

/// Open `url` in the user's default browser (the pop-out button).
pub(crate) fn open_external(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}
