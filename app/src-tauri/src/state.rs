//! The shared [`AppState`]: a thin Tauri adapter over the transport-agnostic
//! [`harness_host::SessionService`], which owns the per-session agent
//! lifecycle (build, resume, cache, evict), turn driving, and the
//! ask/approval round-trips. This file contributes only the Tauri-specific
//! pieces: the [`TauriSink`] that puts protocol events on the webview bus,
//! the native-preview hooks, and the bits of state that are about this
//! window (the console bridge, the preview-attach lock).

use std::path::PathBuf;
use std::sync::Arc;

use harness_host::{EventSink, HostHooks, SessionService};
use harness_protocol::ProtocolEvent;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

pub use harness_host::launch_dir;

/// Tauri state: the shared service plus window-scoped extras. Derefs to the
/// service so commands read `state.dev_servers`, `state.agents`, … directly.
pub struct AppState {
    pub(crate) service: Arc<SessionService>,
    /// The console bridge preview pages beacon their errors to, started on
    /// first preview attach (see `crate::preview::console_bridge`).
    pub(crate) console_bridge: tokio::sync::OnceCell<Arc<harness_preview::ConsoleBridge>>,
    /// Serializes `preview_attach` (see its docs): the frontend calls it at
    /// frame rate during a resize, and webview creation must not race itself.
    pub(crate) preview_attach: Mutex<()>,
}

impl std::ops::Deref for AppState {
    type Target = SessionService;
    fn deref(&self) -> &SessionService {
        &self.service
    }
}

impl AppState {
    /// Wire the service to this app: protocol events emit on the legacy
    /// webview channels, and the preview hooks use the native child-webview
    /// surfaces.
    pub(crate) fn new(
        app: AppHandle,
        initial_project: PathBuf,
        initial_model: String,
        initial_local: Option<String>,
    ) -> Self {
        let sink = Arc::new(TauriSink { app: app.clone() });
        let preview_app = app.clone();
        let lens_app = app.clone();
        let close_app = app.clone();
        let hooks = HostHooks {
            preview_sink: Some(Box::new(move |session| {
                Arc::new(crate::preview::TauriPreviewSink {
                    app: preview_app.clone(),
                    session: session.to_string(),
                })
            })),
            preview_lens: Some(Box::new(move |session| {
                Arc::new(crate::preview::TauriPreviewLens {
                    app: lens_app.clone(),
                    session: session.to_string(),
                })
            })),
            on_session_deleted: Some(Box::new(move |session| {
                crate::preview::close(&close_app, session);
            })),
        };
        let service = Arc::new(
            SessionService::builder(sink)
                .cloud_model(initial_model)
                .local_model(initial_local)
                .active_project(initial_project)
                .hooks(hooks)
                .build(),
        );
        Self {
            service,
            console_bridge: tokio::sync::OnceCell::new(),
            preview_attach: Mutex::new(()),
        }
    }
}

/// Puts protocol events on the webview event bus: the event's dotted tag
/// becomes the legacy channel (`agent.tool_delta` → `agent://tool-delta`) and
/// its fields become the payload — the exact shapes `app/src/lib/ipc.ts`
/// already listens for.
pub(crate) struct TauriSink {
    pub(crate) app: AppHandle,
}

impl EventSink for TauriSink {
    fn emit(&self, event: ProtocolEvent) {
        let channel = event.channel();
        let mut payload = match serde_json::to_value(&event) {
            Ok(value) => value,
            Err(_) => return,
        };
        if let Some(map) = payload.as_object_mut() {
            map.remove("type");
        }
        let _ = self.app.emit(channel.as_str(), payload);
    }
}

/// Open the shared on-disk history store (same DB the agents persist to) —
/// for the read-only stats/export commands that don't need the service.
pub(crate) fn open_history_store() -> Result<harness_store::HistoryStore, String> {
    let path = harness_config::paths::history_db().map_err(|e| e.to_string())?;
    harness_store::HistoryStore::open(path).map_err(|e| e.to_string())
}
