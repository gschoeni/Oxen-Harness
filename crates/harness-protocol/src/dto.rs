//! Command request/response shapes shared by every host transport: the Tauri
//! invoke layer, the HTTP server's routes, and client SDKs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One selectable choice within a [`Question`]. Serde-compatible with
/// `harness_tools::Choice` (pinned by a wire test).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Choice {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// A structured question the model asked via `ask_user_question`.
/// Serde-compatible with `harness_tools::Question`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    pub options: Vec<Choice>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// The user's answer to one [`Question`]. Serde-compatible with
/// `harness_tools::QuestionAnswer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAnswer {
    /// The question's `header`, echoed back for context.
    pub header: String,
    /// The question text, echoed back for context.
    pub question: String,
    /// The selected option label(s), or the user's free-text answer.
    pub selected: Vec<String>,
}

/// The user's reply to one approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalAnswer {
    /// "once" | "session" | "project" | "trash" | "bypass" | "deny".
    pub decision: String,
    /// The user's own words when denying (sent back to the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A session's live vitals — what a UI needs to render its header/meters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInfo {
    pub model: String,
    pub workspace: String,
    pub session_id: String,
    /// Cumulative tokens used in this session.
    pub tokens_used: usize,
    /// Tokens the current transcript occupies (context-window fill).
    pub context_tokens: usize,
    /// The model's effective context window.
    pub context_window: usize,
    /// The context-compression mode this session's agent runs with
    /// ("off"/"audit"/"on").
    pub compression_mode: String,
}

/// A resumed session: its info plus the verbatim transcript to re-render.
/// When `running` is true the chat is mid-turn and couldn't be read;
/// `messages` is empty and the client keeps whatever it already streamed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionView {
    pub info: SessionInfo,
    pub messages: Vec<serde_json::Value>,
    pub running: bool,
}

/// A request to run one user turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnRequest {
    pub prompt: String,
    /// Paths of attachments readable by the host (dropped files on the
    /// desktop; upload-endpoint results over HTTP).
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// A completed turn's final assistant text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnResponse {
    pub text: String,
}

/// A message for the session's *running* turn (mid-turn steering): delivered
/// into the turn at its next safe point rather than queued for after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterjectRequest {
    pub text: String,
}

/// One subagent lane of a session, running or finished (see
/// `GET /v1/sessions/{id}/agents`). A running lane has a status of
/// `"running"` and no record yet; a finished one carries the typed record
/// its parent read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSummary {
    /// The lane's id (its session id).
    pub id: String,
    #[serde(default)]
    pub parent: String,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub stop: Option<String>,
    #[serde(default)]
    pub has_patch: bool,
    pub label: String,
    pub fleet: String,
    /// `running`, `done`, `partial`, or `failed`.
    pub status: String,
    /// The reply's head, or the error, once finished.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tokens: usize,
    #[serde(default)]
    pub rounds: u32,
    /// Seconds running so far (running lanes only).
    #[serde(default)]
    pub elapsed_secs: u64,
    /// Unix seconds the lane was created.
    #[serde(default)]
    pub created_at: i64,
}

/// One background shell task of a session (`GET /v1/sessions/{id}/tasks`,
/// and the `tasks.changed` event): running or ended, for how long, and the
/// last line it printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskSummary {
    pub id: u64,
    pub command: String,
    pub running: bool,
    /// The exit code once ended; absent while running or when it died on a
    /// signal.
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub killed: bool,
    pub elapsed_secs: u64,
    #[serde(default)]
    pub last_line: String,
}

/// One image/video generation of a project (`media.changed`, and the
/// media listing commands): a manifest row of `harness_media`, mirrored on
/// the wire. Paths are project-relative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaItem {
    /// The hub's generation id.
    pub id: String,
    /// The chat that asked for it.
    pub session: String,
    /// Groups the outputs of one tool call.
    #[serde(default)]
    pub batch: String,
    /// 1-based position within the batch.
    #[serde(default)]
    pub index: u32,
    /// `image` or `video`.
    pub kind: String,
    pub model: String,
    pub prompt: String,
    /// The request parameters sent (minus prompt and reference data).
    #[serde(default)]
    pub params: serde_json::Value,
    /// Project-relative paths of the reference copies used.
    #[serde(default)]
    pub refs: Vec<String>,
    /// Project-relative path of the saved output, once there is one.
    #[serde(default)]
    pub path: Option<String>,
    /// Project-relative poster frame for a video, when one was extracted.
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub duration_secs: Option<f64>,
    /// The catalog's estimate for this one output, in USD.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// `queued`, `processing`, `succeeded`, `failed`, `cancelled`, `timed_out`.
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default)]
    pub completed_at: Option<i64>,
    /// The item this one varies, upscales, or animates.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub seed: Option<serde_json::Value>,
}

/// One reference on its way to the hub for a generation (`media.changed`
/// `uploads`): a progress-bar row. Live-only — a finished upload leaves the
/// list once its generation is recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaUpload {
    pub id: String,
    pub session: String,
    /// The chip label it came from (`[Image #1]`), when it did.
    #[serde(default)]
    pub label: Option<String>,
    pub filename: String,
    /// `image`, `video`, `audio`.
    pub kind: String,
    pub bytes_sent: u64,
    pub bytes_total: u64,
    /// `uploading`, `presigning`, `done`, `reused`, `failed`.
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub started_at: i64,
}

/// One image/video model of the hub catalog, for pickers (`GET
/// /v1/media/models`, the desktop's Settings → Media).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaModelSummary {
    pub id: String,
    /// `image` or `video`.
    pub kind: String,
    /// `$0.01/image`, `$0.08–$0.17/s`, `unpriced`.
    pub price: String,
    #[serde(default)]
    pub developer: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// Input modalities (`text`, `image`, `video`, `audio`).
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// Whether a running turn accepted the interjection. `accepted: false` means
/// no turn was in flight — send the text as an ordinary prompt instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterjectResponse {
    pub accepted: bool,
}

/// What a code-review run resolved to. `status` is `"ok"`, `"nothing"` (the
/// target had no changes), or `"cancelled"`; on `"ok"` the user/assistant
/// pair is already persisted to the session, so the client appends it to the
/// thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub status: String,
    pub user: String,
    pub assistant: String,
    pub findings: usize,
    /// Estimated tokens spent across every reviewer agent in the pipeline.
    pub tokens_used: usize,
}

/// What a verification-loop run resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoopResult {
    pub succeeded: bool,
    pub iterations: u32,
    pub summary: String,
}

/// A compact plan reading — "3/5, currently: Running tests". Serde-compatible
/// with `harness_tools::PlanSnapshot` (pinned by a wire test), which is the
/// shape the agent persists per session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanProgress {
    /// Items marked completed.
    pub done: usize,
    /// Items in the plan overall.
    pub total: usize,
    /// The in-progress item's present-continuous label, when one is underway.
    #[serde(default)]
    pub active: Option<String>,
}

/// One named stage on a thread's charted journey. Serde-compatible with
/// `harness_tools::Waypoint` (pinned by a wire test).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrailWaypoint {
    /// Stage name, e.g. "define", "implement".
    pub name: String,
    /// `"ahead"` | `"current"` | `"done"`.
    pub status: String,
}

/// The journey the model charted for a session via `update_trail`: a
/// self-chosen thread title plus its macro stages. Serde-compatible with
/// `harness_tools::TrailSnapshot` (pinned by a wire test).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrailProgress {
    /// The model-chosen thread title.
    pub title: String,
    /// The route, in order.
    pub waypoints: Vec<TrailWaypoint>,
}

/// The mark a settled ("tied off") thread carries in the Ledger: when it was
/// closed and, optionally, the user's one-line closing note ("shipped as
/// PR #42"). Absence of this state is what "open" means — there is no
/// separate open/closed flag to fall out of sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettleState {
    /// Unix seconds when the thread was tied off.
    pub settled_at: i64,
    /// The user's closing note; empty when they skipped it.
    #[serde(default)]
    pub note: String,
}

/// A request to settle (tie off) a session's thread in the Ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettleRequest {
    /// Optional one-line closing note.
    #[serde(default)]
    pub note: String,
}

/// A request to give a session a name of the user's choosing. A blank name
/// clears it, so the session titles itself by its first message again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenameRequest {
    pub title: String,
}

/// One thread as the Ledger renders it: a native session with everything that
/// decides where its wagon sits — freshness, plan progress, whether it stopped
/// mid-turn, and whether it has been tied off. Workspace-level facts (git
/// state, project names) deliberately are NOT here: they belong to the
/// workspace, not the thread, and travel separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LedgerEntry {
    pub id: String,
    pub workspace: String,
    pub model: String,
    /// Unix seconds the session was created.
    pub created_at: i64,
    /// Unix seconds of the newest message (session creation if none).
    pub last_activity_at: i64,
    /// The first user message's text — the thread's title.
    pub title: String,
    /// The opening of the newest assistant message — the thread's last word,
    /// shown on its waystation card. Empty when the model never replied.
    #[serde(default)]
    pub last_reply: String,
    pub message_count: i64,
    /// The stored transcript stops on a user message or tool result — a reply
    /// never arrived. Combined with `running` on the snapshot: mid-turn and
    /// not running means the thread was left dangling.
    pub mid_turn: bool,
    /// Latest plan reading, when the thread ever laid one out.
    #[serde(default)]
    pub plan: Option<PlanProgress>,
    /// The journey the model charted via `update_trail`, when it has. Its
    /// `title` supersedes the first-user-message `title` for display.
    #[serde(default)]
    pub trail: Option<TrailProgress>,
    /// Present once the thread has been tied off.
    #[serde(default)]
    pub settle: Option<SettleState>,
    /// Training-data curation: `""` (unreviewed), `"kept"`, or `"rejected"`.
    #[serde(default)]
    pub review_status: String,
    /// Unix seconds the user last looked at this thread (opened its chat or
    /// watched its turn end); `0` when never recorded. Activity newer than
    /// this is "finished while you were away" — per thread, so it stays
    /// flagged until the user actually opens it.
    #[serde(default)]
    pub seen_at: i64,
}

/// Everything the Ledger board needs, in one read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LedgerSnapshot {
    /// Every native thread, newest activity first.
    pub entries: Vec<LedgerEntry>,
    /// Session ids with work in flight right now (a turn, review, or loop) —
    /// read from the host's authoritative in-flight registry, so it is correct
    /// even after a UI restart.
    pub running: Vec<String>,
    /// Unix seconds the Ledger was last marked seen; 0 on first visit. The
    /// board renders "since you left" against this.
    pub last_seen: i64,
}
