//! The link-browser pane: a native child webview showing a web page the user
//! clicked in the chat, positioned over the React layout's placeholder — the
//! same shape as [`crate::preview`], but app-wide (one webview, not one per
//! session) and pointed at arbitrary http(s) pages instead of localhost.
//!
//! It exists so a clicked link never navigates the MAIN webview: the app's
//! whole UI lives there, and navigating it away leaves the user staring at a
//! full-window web page with no way back. Clicks are intercepted in the
//! frontend (see `app/src/lib/links.ts`); the crate root's `nav-guard` plugin
//! backstops anything that slips through by cancelling the navigation and
//! handing the URL to this pane via a `browser://open` event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::preview::open_external;

/// The single browser webview's label.
pub(crate) const LABEL: &str = "link-browser";

/// The URL the pane was last pointed at by [`attach`] — same rationale as the
/// preview's tracked URL: the page may navigate itself (links within the pane
/// are allowed), and comparing against the live URL would re-navigate on every
/// layout pass.
static NAVIGATED: StdMutex<Option<String>> = StdMutex::new(None);

/// Bumped by every [`detach`]. A slow first attach that finishes *after* an
/// overlay opened must not `show()` the native view over it — native webviews
/// paint above all DOM.
static DETACH_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Show the browser webview at `bounds` (CSS pixels), creating it on first
/// use, navigating it when the frontend asks for a different URL, and
/// repositioning it otherwise.
pub(crate) fn attach(
    app: &AppHandle,
    url: &str,
    bounds: &crate::preview::Bounds,
) -> Result<(), String> {
    let target: Url = url.parse().map_err(|e| format!("bad url: {e}"))?;
    if !matches!(target.scheme(), "http" | "https") {
        return Err(format!("refusing to open non-web URL: {url}"));
    }
    let epoch = DETACH_EPOCH.load(Ordering::SeqCst);

    let position = LogicalPosition::new(bounds.x, bounds.y);
    let size = LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0));
    let show = |webview: &tauri::webview::Webview<tauri::Wry>| {
        // A detach raced us (an overlay opened while we were working) —
        // position the view but leave it hidden; the next attach shows it.
        if DETACH_EPOCH.load(Ordering::SeqCst) == epoch {
            let _ = webview.show();
        }
    };

    if let Some(webview) = app.webviews().get(LABEL).cloned() {
        let mut navigated = NAVIGATED.lock().unwrap();
        if navigated.as_deref() != Some(url) {
            webview
                .navigate(target)
                .map_err(|e| format!("navigate browser pane: {e}"))?;
            *navigated = Some(url.to_string());
        }
        drop(navigated);
        webview.set_position(position).map_err(|e| e.to_string())?;
        webview.set_size(size).map_err(|e| e.to_string())?;
        show(&webview);
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    // The pane is a small browser: following links within it is fine, but
    // anything that isn't a web page (mailto:, custom app schemes) goes to the
    // system handler instead of dead-ending a WKWebView.
    let builder =
        WebviewBuilder::new(LABEL, WebviewUrl::External(target)).on_navigation(move |url: &Url| {
            let allowed = matches!(url.scheme(), "http" | "https" | "about");
            if !allowed {
                let _ = open_external(url.as_str());
            }
            allowed
        });
    window
        .add_child(builder, position, size)
        .map_err(|e| format!("create browser webview: {e}"))?;
    *NAVIGATED.lock().unwrap() = Some(url.to_string());
    // Creation shows the webview; if an overlay opened meanwhile, hide it.
    if DETACH_EPOCH.load(Ordering::SeqCst) != epoch {
        detach(app);
    }
    Ok(())
}

/// Hide the browser webview (pane unmounted, overlay opened, tab switched).
pub(crate) fn detach(app: &AppHandle) {
    DETACH_EPOCH.fetch_add(1, Ordering::SeqCst);
    if let Some(webview) = app.webviews().get(LABEL) {
        let _ = webview.hide();
    }
}

/// Destroy the browser webview entirely (the user closed the pane) — a hidden
/// webview keeps its page alive, and a closed pane shouldn't hold one.
pub(crate) fn close(app: &AppHandle) {
    *NAVIGATED.lock().unwrap() = None;
    if let Some(webview) = app.webviews().get(LABEL) {
        let _ = webview.close();
    }
}

/// Reload the pane's current page (toolbar refresh button).
pub(crate) fn reload(app: &AppHandle) {
    if let Some(webview) = app.webviews().get(LABEL) {
        let _ = webview.eval("location.reload()");
    }
}

/// The URL the pane is actually showing right now (it may have navigated
/// itself since [`attach`]) — used by the pop-out button so the system browser
/// opens the page the user is looking at, not the link they started from.
pub(crate) fn current_url(app: &AppHandle) -> Option<String> {
    let webview = app.webviews().get(LABEL).cloned()?;
    webview.url().ok().map(|u| u.to_string())
}
