//! Every payload the backend emits to the webview, in one place so the wire
//! format the frontend listens for (see `app/src/lib/ipc.ts`) is auditable at
//! a glance. Each struct documents the event channel it rides on; the structs
//! are grouped by feature. Emitting stays with the code that owns the moment
//! (turns, bridges, model installs) — this module owns only the shapes.

use harness_tools::Question;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

// --- Turn streaming --------------------------------------------------------

/// A streamed assistant token, tagged with the session it belongs to so the UI
/// can route it to the right chat thread (even one running in the background).
#[derive(Clone, Serialize)]
pub(crate) struct TokenPayload {
    pub(crate) session: String,
    pub(crate) token: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct ToolEventPayload {
    pub(crate) session: String,
    pub(crate) phase: &'static str,
    pub(crate) name: String,
    pub(crate) detail: String,
}

/// An `agent://tool-delta` payload: an incremental fragment of a tool call's
/// JSON arguments, so the UI can stream the in-progress content (a file being
/// written, a canvas document being authored).
#[derive(Clone, Serialize)]
pub(crate) struct ToolDeltaPayload {
    pub(crate) session: String,
    pub(crate) name: String,
    pub(crate) delta: String,
}

/// An `agent://usage` payload: the session's cumulative token count plus current
/// context fill, emitted around each model call within a turn so the UI tracks
/// usage live. (The all-time grand total is a separate, turn-end concern.)
#[derive(Clone, Serialize)]
pub(crate) struct UsagePayload {
    pub(crate) session: String,
    pub(crate) tokens_used: usize,
    pub(crate) context_tokens: usize,
    pub(crate) context_window: usize,
    pub(crate) prompt_tokens_used: usize,
    pub(crate) completion_tokens_used: usize,
}

/// `agent://compacted` payload — the transcript was trimmed to fit the window,
/// with a short human-readable note for the thread.
#[derive(Clone, Serialize)]
pub(crate) struct CompactedPayload {
    pub(crate) session: String,
    pub(crate) detail: String,
}

/// `agent://retry` payload — a model call hit a transient provider/network
/// error and is being retried with backoff. Surfaced as a thread notice so the
/// pause reads as a hiccup (with the error for debugging), not a hang.
#[derive(Clone, Serialize)]
pub(crate) struct RetryPayload {
    pub(crate) session: String,
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) delay_ms: u64,
    pub(crate) error: String,
}

/// `agent://compression` payload — stale tool output was compressed before a
/// model call (`mode: "on"`), or its would-be savings were measured without
/// changing the request (`mode: "audit"`). Emitted per model call within a
/// turn, so the UI updates counters rather than appending thread notices.
#[derive(Clone, Serialize)]
pub(crate) struct CompressionPayload {
    pub(crate) session: String,
    pub(crate) mode: String,
    pub(crate) saved_tokens: usize,
    pub(crate) total_saved_tokens: usize,
    pub(crate) results_compressed: usize,
}

// --- Questions & canvas ----------------------------------------------------

#[derive(Clone, Serialize)]
pub(crate) struct QuestionPayload {
    pub(crate) id: String,
    pub(crate) questions: Vec<Question>,
}

/// A session-only event payload (e.g. `agent://canvas-writing`).
#[derive(Clone, Serialize)]
pub(crate) struct SessionPayload {
    pub(crate) session: String,
}

/// The `agent://canvas` payload: a document for the UI's side panel, tagged with
/// the session it belongs to so a background chat's canvas doesn't pop into view.
#[derive(Clone, Serialize)]
pub(crate) struct CanvasPayload {
    pub(crate) session: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) format: String,
    pub(crate) language: Option<String>,
    pub(crate) content: String,
}

// --- Live preview ------------------------------------------------------------

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

// --- Local models ----------------------------------------------------------

/// A `local://status` payload: a phase of bringing a local model online, so the
/// UI can show meaningful progress while switching to it.
#[derive(Clone, Serialize)]
pub(crate) struct LocalStatusPayload {
    pub(crate) model: String,
    /// `"starting"` (runtime/GPU init), `"loading"` (reading weights),
    /// `"ready"`, or `"error"` (the load ended without a server).
    pub(crate) phase: &'static str,
}

/// Report a local-model load phase to the UI (`local://status`).
pub(crate) fn emit_local_status(app: &AppHandle, model: &str, phase: &'static str) {
    let _ = app.emit(
        "local://status",
        LocalStatusPayload {
            model: model.to_string(),
            phase,
        },
    );
}

/// The `start_with_context` progress callback that streams load phases to the
/// UI — shared by the explicit model switch (`use_local_model`) and the lazy
/// restore of a persisted local selection on first use after launch
/// (`ensure_local_server`), so both render the same loading state.
pub(crate) fn local_status_emitter(
    app: &AppHandle,
    model: &str,
) -> impl FnMut(harness_local::LoadPhase) {
    let app = app.clone();
    let model = model.to_string();
    move |phase| {
        emit_local_status(
            &app,
            &model,
            match phase {
                harness_local::LoadPhase::Starting => "starting",
                harness_local::LoadPhase::LoadingModel => "loading",
                harness_local::LoadPhase::Ready => "ready",
            },
        )
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct DownloadEvent {
    pub(crate) id: String,
    pub(crate) downloaded: u64,
    pub(crate) total: Option<u64>,
    pub(crate) fraction: Option<f64>,
}

// --- Code review -----------------------------------------------------------

/// A `review://progress` payload: which pipeline step a running code review is
/// on — and, for a fan-out step, the parallel lanes it runs — so the chat can
/// show a live progress card.
#[derive(Clone, Serialize)]
pub(crate) struct ReviewProgressPayload {
    pub(crate) session: String,
    pub(crate) step: String,
    pub(crate) index: usize,
    pub(crate) total: usize,
    /// Lane labels for this step, in order. More than one = a fan-out.
    pub(crate) agents: Vec<String>,
}

/// A `review://token` payload: streamed text from the current review step's
/// agent (the card's live activity feed).
#[derive(Clone, Serialize)]
pub(crate) struct ReviewTokenPayload {
    pub(crate) session: String,
    pub(crate) token: String,
}

/// A `review://tool` payload: a tool the current review step's agent invoked.
#[derive(Clone, Serialize)]
pub(crate) struct ReviewToolPayload {
    pub(crate) session: String,
    pub(crate) name: String,
}

/// What `run_code_review` resolves with. `status` is `"ok"`, `"nothing"` (the
/// target had no changes), or `"cancelled"`; on `"ok"` the user/assistant pair
/// is already persisted to the session, so the UI appends it to the thread.
#[derive(Clone, Serialize)]
pub(crate) struct CodeReviewResult {
    pub(crate) status: &'static str,
    pub(crate) user: String,
    pub(crate) assistant: String,
    pub(crate) findings: usize,
    /// Estimated tokens spent across every reviewer agent in the pipeline.
    pub(crate) tokens_used: usize,
}

// --- Fleet lanes -----------------------------------------------------------

/// A `fleet://started` payload: a fleet of parallel subagents is spinning up
/// in `session` — from a review fan-out step or the model's `spawn_agents`
/// call alike. `agents` is the lane labels, in order.
#[derive(Clone, Serialize)]
pub(crate) struct FleetStartedPayload {
    pub(crate) session: String,
    pub(crate) agents: Vec<String>,
    /// `"review"` (a pipeline step) or `"turn"` (the model's spawn_agents).
    pub(crate) source: &'static str,
}

/// A `fleet://agent` payload: one lane changed state.
#[derive(Clone, Serialize)]
pub(crate) struct FleetAgentPayload {
    pub(crate) session: String,
    pub(crate) agent: usize,
    pub(crate) name: String,
    /// `"started"`, `"done"`, or `"failed"`.
    pub(crate) phase: &'static str,
    pub(crate) tokens: usize,
    /// The lane's truncated reply or error (set on done/failed).
    pub(crate) summary: String,
}

/// A `fleet://agent-activity` payload: what one lane is doing right now —
/// streamed text, a tool invocation, or a token-count update.
#[derive(Clone, Serialize)]
pub(crate) struct FleetActivityPayload {
    pub(crate) session: String,
    pub(crate) agent: usize,
    /// `"token"` (append text), `"tool"` (replace with a tool line), or
    /// `"tokens"` (update the counter).
    pub(crate) kind: &'static str,
    pub(crate) text: String,
    pub(crate) tokens: Option<usize>,
}
