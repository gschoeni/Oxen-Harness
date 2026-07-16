//! The few payloads the desktop still emits *outside* the shared protocol
//! stream. Everything agent/turn/fleet/review-shaped now rides
//! `harness_protocol::ProtocolEvent` through `crate::state::TauriSink`; what
//! remains here is strictly window business — the native preview pane and the
//! nav-guard's link-browser handoff.

use serde::Serialize;

/// A `preview://status` payload: the session's dev server changed lifecycle
/// phase (starting/ready/error/stopped). The flattened status carries the
/// name, command, URL, port, and any error message.
#[derive(Clone, Serialize)]
pub(crate) struct PreviewStatusPayload {
    pub(crate) session: String,
    #[serde(flatten)]
    pub(crate) status: harness_preview::PreviewStatus,
}

/// A `preview://console` payload: the preview page hit a JavaScript error —
/// drives the pane's "Fix it" banner.
#[derive(Clone, Serialize)]
pub(crate) struct PreviewConsolePayload {
    pub(crate) session: String,
    pub(crate) text: String,
}

/// A `browser://open` payload: the navigation guard cancelled a main-webview
/// navigation (a link click nothing intercepted) — the UI should show `url`
/// in the link-browser side panel instead.
#[derive(Clone, Serialize)]
pub(crate) struct BrowserOpenPayload {
    pub(crate) url: String,
}
