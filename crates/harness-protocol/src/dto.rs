//! Command request/response shapes shared by every host transport: the Tauri
//! invoke layer, the HTTP server's routes, and client SDKs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProtocolEvent;

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
    /// What the running turn is parked on right now — an `agent.question`
    /// or `agent.approval_request` the client must answer — replayed here so
    /// a client that missed the original event (a reloaded webview, a fresh
    /// HTTP subscriber) can still answer it. Empty when nothing is pending.
    #[serde(default)]
    pub pending: Vec<ProtocolEvent>,
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
    /// The turn within that chat: the persisted `seq` of the user message
    /// that started it. Absent on older rows.
    #[serde(default)]
    pub turn_seq: Option<i64>,
    /// The model's id for the tool call that made it.
    #[serde(default)]
    pub call_id: Option<String>,
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
    /// Where each reference came from, in request order (the provenance
    /// behind `refs`).
    #[serde(default)]
    pub sources: Vec<MediaSource>,
    /// The prompt as the agent wrote it, when rewriting reference labels
    /// changed what the hub got (`prompt`).
    #[serde(default)]
    pub agent_prompt: Option<String>,
    /// The hub's completed generation record, verbatim.
    #[serde(default)]
    pub provider: Option<serde_json::Value>,
}

/// One reference's provenance on a `MediaItem`: the stored copy, the file
/// it was made from, and how that file got into the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaSource {
    /// The project-relative copy under `refs/` (the matching `refs` entry).
    pub path: String,
    /// `attachment` (dropped into the chat), `generation` (an earlier
    /// output of the library, named by `generation`), or `file` (any other
    /// project file).
    pub origin: String,
    /// The chip label the request used (`[Image #1]`), when it came from
    /// the chat.
    #[serde(default)]
    pub label: Option<String>,
    /// The original file: absolute for an attachment from outside the
    /// project, project-relative otherwise.
    pub source: String,
    /// The library item whose output this is, for a `generation` origin.
    #[serde(default)]
    pub generation: Option<String>,
    /// `image`, `video`, or `audio`.
    pub kind: String,
    /// SHA-256 of the bytes, hex.
    pub sha256: String,
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

/// A request to give a session a name of the user's choosing. A blank name
/// clears it, so the session titles itself by its first message again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenameRequest {
    pub title: String,
}

/// One session as an overview surface sees it: what decides whether it needs
/// the user — freshness, whether it stopped mid-turn, and whether the user
/// marked it finished. Workspace-level facts (git state, project names)
/// deliberately are NOT here: they belong to the workspace, not the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadEntry {
    pub id: String,
    pub workspace: String,
    pub model: String,
    /// Unix seconds the session was created.
    pub created_at: i64,
    /// Unix seconds of the newest message (session creation if none).
    pub last_activity_at: i64,
    /// The first user message's text — the thread's title.
    pub title: String,
    /// The opening of the newest assistant message — the thread's last word.
    /// Empty when the model never replied.
    #[serde(default)]
    pub last_reply: String,
    pub message_count: i64,
    /// The stored transcript stops on a user message or tool result — a reply
    /// never arrived. Combined with `running` on the snapshot: mid-turn and
    /// not running means the thread was left dangling.
    pub mid_turn: bool,
    /// Unix seconds the user marked this thread finished; `0` while open.
    /// The one piece of human-authored state: set by the finish route,
    /// cleared by reopening (or by the thread running again).
    #[serde(default)]
    pub finished_at: i64,
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

/// Every native thread plus which ones are running, in one read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSnapshot {
    /// Every native thread, newest activity first.
    pub entries: Vec<ThreadEntry>,
    /// Session ids with work in flight right now (a turn or a review) —
    /// read from the host's authoritative in-flight registry, so it is correct
    /// even after a UI restart.
    pub running: Vec<String>,
}

// ---- the codebase study game -------------------------------------------------

/// One territory of a project — a crate, a feature directory, the docs —
/// with the player's decayed mastery of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StudyTerritory {
    /// Stable id: the territory's path relative to the workspace root.
    pub id: String,
    /// Display name (the last path segment, or "docs").
    pub name: String,
    /// Mastery right now, 0..1, after spaced-repetition decay.
    pub mastery: f32,
    /// Questions answered in this territory, all time.
    pub answered: u32,
    /// Of those, answered fully correctly.
    pub correct: u32,
    /// Unix seconds of the last answer here, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_answered_at: Option<i64>,
    /// How much mastery has faded since its peak, 0..1 — what the game turns
    /// into "your memory of X is fading" events.
    pub faded: f32,
}

/// The player's understanding of one project: the per-territory mastery
/// and the level derived from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StudyProfile {
    /// The project key the progress is filed under.
    pub project: String,
    /// The workspace root the territories are relative to.
    pub workspace: String,
    /// Understanding across the whole project, 0..100.
    pub understanding: f32,
    /// The level derived from `understanding` (1 and up).
    pub level: u32,
    /// Questions answered all time, across territories.
    pub answered: u32,
    pub territories: Vec<StudyTerritory>,
}

/// One quiz question. The correct answer stays server-side: the client
/// submits an answer and receives a [`StudyGrade`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StudyQuestion {
    pub id: String,
    /// The territory id this question tests.
    pub territory: String,
    /// `"multiple_choice"`, `"true_false"`, or `"free_text"`.
    pub kind: String,
    pub prompt: String,
    /// The options for a choice question (2-4 entries); empty for free text.
    #[serde(default)]
    pub options: Vec<String>,
    /// Where the answer lives, relative to the workspace root.
    pub source_path: String,
    /// The lines the answer lives on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_lines: Option<(u32, u32)>,
    /// A short excerpt of the source, shown as the hint.
    #[serde(default)]
    pub source_excerpt: String,
    /// 1 (recall) to 3 (reasoning).
    pub difficulty: u8,
    /// Whether this question came from the cache rather than fresh generation.
    #[serde(default)]
    pub cached: bool,
}

/// A batch of questions for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StudyBatch {
    /// The mode the batch was drawn for.
    pub mode: String,
    pub questions: Vec<StudyQuestion>,
    /// The territory the batch focuses on (its display name).
    pub territory: String,
    /// Tokens the generation spent (0 when everything came from the cache).
    pub tokens_used: usize,
    /// The model that wrote the fresh questions, if any were generated.
    #[serde(default)]
    pub model: String,
}

/// What a client asks for when it needs questions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StudyBatchRequest {
    /// `"expedition"`, `"fresh_tracks"`, `"ride_along"`, or `"review"`.
    pub mode: String,
    /// How many questions (capped server-side).
    #[serde(default)]
    pub count: Option<usize>,
    /// Question ids already used this run, so a batch never repeats them.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// A submitted answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StudyAnswerRequest {
    pub question_id: String,
    /// The chosen option index as text for choice questions, or the typed
    /// answer for free text.
    pub answer: String,
    /// Whether the hint was revealed before answering.
    #[serde(default)]
    pub hint_used: bool,
}

/// How an answer was judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StudyGrade {
    /// `"full"`, `"partial"`, or `"wrong"`.
    pub verdict: String,
    /// One line of correction or confirmation for the result card.
    pub feedback: String,
    /// The correct answer, spelled out.
    pub correct_answer: String,
    /// Why, in a sentence or two.
    pub explanation: String,
}

/// The reply to a submitted answer: the grade plus the profile after it
/// was recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StudyAnswerResult {
    pub grade: StudyGrade,
    pub profile: StudyProfile,
    /// Tokens a free-text grade spent (0 for choice questions).
    pub tokens_used: usize,
}
