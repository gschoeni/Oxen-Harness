// Wire types — kept in sync with the Tauri commands in src-tauri/src/lib.rs and
// the Rust crates they return. Field names match Rust serde output (snake_case
// unless a struct renames, e.g. `multiSelect`).
//
// The chat-message wire types (ChatMessage/MessageContent/ContentPart/…) are
// GENERATED from the Rust source of truth in crates/harness-llm/src/types.rs.
// Don't hand-edit them; regenerate bindings.ts with:
//   cargo test -p harness-llm --features ts -- --ignored generate_bindings
import type {
  ChatMessage,
  MessageContent,
  ContentPart,
  ImageUrl,
  FileData,
  ToolCall,
  FunctionCall,
} from "./bindings";
export type {
  ChatMessage,
  MessageContent,
  ContentPart,
  ImageUrl,
  FileData,
  ToolCall,
  FunctionCall,
};

export interface SessionInfo {
  model: string;
  workspace: string;
  session_id: string;
  /** Cumulative tokens used in this session (drives the dashboard's count). */
  tokens_used: number;
  /** Tokens the current transcript occupies (how full the context window is). */
  context_tokens: number;
  /** The model's effective context window, for a "% of context" readout. */
  context_window: number;
  /** The context-compression mode this session's agent was built with —
   *  drives the TokenMeter's armed indicator. */
  compression_mode: CompressionMode;
  /** The permission mode this chat's gate is in right now: the saved default
   *  unless the chat switched its own (the composer's picker, or "Dangerously
   *  allow everything" on an approval). */
  permission_mode: PermissionMode;
}

/** A chat's training-data review status: unreviewed (""), kept, or rejected. */
export type ReviewStatus = "" | "kept" | "rejected";

export interface SessionSummary {
  id: string;
  workspace: string;
  model: string;
  created_at: number;
  title: string | null;
  message_count: number;
  /** Whether this chat is kept/rejected for the fine-tuning dataset (else ""). */
  review_status: ReviewStatus;
  /** Where the chat came from: "" for native chats, else the import source
   *  ("claude-code" | "cursor"). Imported chats are review-only — they never
   *  resume as live agents. */
  source: string;
}

/** One importable source of external conversations (Training Data page). */
export interface ImportSourceStatus {
  source: string;
  /** Conversations found in the tool's local logs (drafts included). */
  available: number;
  /** Sessions already imported from this source. */
  imported: number;
}

/** What one import pass did: new, refreshed (grown at the source), unchanged. */
export interface ImportReport {
  imported: number;
  updated: number;
  skipped: number;
}

export interface SessionView {
  info: SessionInfo;
  messages: ChatMessage[];
  /** True when the chat is mid-turn and couldn't be read; keep the live thread. */
  running: boolean;
  /** What the running turn is parked on — a question or approval whose event
   *  this client may never have seen (it arrived before a reload). Replayed
   *  so the card can be shown again and the turn answered. */
  pending?: PendingRoundTrip[];
}

/** A host round-trip the running turn is waiting on, as the backend replays
 *  it on resume: the same payload the live event carries, tagged by kind. */
export type PendingRoundTrip =
  | ({ type: "agent.question" } & QuestionPayload)
  | ({ type: "agent.approval_request" } & ApprovalRequestEvent);

/** `turn://completed` / `turn://failed` — a turn's end as seen on the event
 *  stream. The client that started the turn learns this from its own
 *  promise; these let a client that *rejoined* a running turn (after a
 *  reload) learn it too. */
export interface TurnEndedEvent {
  session: string;
  /** The final reply (completed) — absent when the turn failed. */
  text?: string;
  /** The failure (failed) — absent when the turn completed. */
  error?: string;
}

/** A tool definition (JSON schema) as advertised to the model on each call.
 *  Loosely typed — the schema is provider JSON, inspected raw in the dev view. */
export interface ToolDefinition {
  type?: string;
  function?: { name?: string; description?: string; parameters?: unknown };
}

export type ProjectContextKind = "text" | "pdf" | "image";

export interface ProjectContext {
  path: string;
  name: string;
  kind: ProjectContextKind;
  size_bytes: number;
}

/** A project is a working directory plus durable, repository-local guidance. */
export interface Project {
  path: string;
  name: string;
  description: string;
  instructions: string;
  context: ProjectContext[];
  /** The project's remote Oxen repository on the hub as `namespace/name`; null when none is set. */
  remote_repo: string | null;
  session_count: number;
  active: boolean;
  /** Unix seconds of the newest message in any of this project's chats; null when it has no history. */
  last_used_at: number | null;
}

/** One chat as the threads overview sees it: a native session plus the
 *  facts that decide whether it needs the user. */
export interface ThreadEntry {
  id: string;
  workspace: string;
  model: string;
  created_at: number;
  /** Unix seconds of the newest message (session creation if none). */
  last_activity_at: number;
  /** The first user message's text — the thread's title. */
  title: string;
  /** The opening of the newest assistant message. Empty when the model
   *  never replied. */
  last_reply: string;
  message_count: number;
  /** The stored transcript stops on a user message or tool result — a reply
   *  never arrived. Mid-turn and not running means left dangling. */
  mid_turn: boolean;
  /** Unix seconds the user marked this thread finished; 0 while open. */
  finished_at: number;
  /** Training-data curation: "" (unreviewed), "kept", or "rejected". */
  review_status: ReviewStatus;
  /** Unix seconds the user last looked at this thread (opened its chat or
   *  watched its turn end); 0 when never recorded. Activity newer than this
   *  is "finished while you were away" — per thread, so a loose end stays
   *  flagged until the user actually opens it. */
  seen_at: number;
}

/** Every native thread plus the in-flight session ids, in one read. */
export interface ThreadSnapshot {
  /** Every native thread, newest activity first. */
  entries: ThreadEntry[];
  /** Session ids with work in flight (a turn or review) — from the
   *  host's authoritative registry, correct even after a UI restart. */
  running: string[];
}

export interface StartProjectInput {
  name: string;
  description: string;
  directory: string;
  createDirectory: boolean;
}

/** A model staged for the fresh chat started from a project home. */
export interface StartupModelChoice {
  id: string;
  label: string;
  local: boolean;
}

// ---- connection settings ---------------------------------------------------

export interface ConnectionView {
  /** Effective host in use — the saved override, else the resolved env/default. */
  host: string;
  /** Effective API key in use — the override, else what resolves from
   *  OXEN_API_KEY / the `oxen` CLI login (empty if nothing resolves). */
  api_key: string;
  /** Effective Brave Search API key enabling web search (empty = off). */
  brave_api_key: string;
  /** Default Oxen host, shown as the host field placeholder. */
  default_host: string;
  /** Whether any API key resolved for the current host. */
  env_key_available: boolean;
}

// ---- cloud models ----------------------------------------------------------

/** A cloud model in the user-curated catalog. `id` is sent to the inference
 *  API; `name` is a friendly label; `selected` is the current default. */
export interface CloudModel {
  id: string;
  name: string;
  selected: boolean;
}

/** Per-token USD rates for a token-billed hosted model (per single token —
 *  multiply by 1e6 for the conventional $/M display). */
export interface ModelPricing {
  input_cost_per_token: number;
  output_cost_per_token: number;
}

/** A hit from the configured endpoint's hosted model catalog. */
export interface OxenModelHit {
  id: string;
  name: string;
  developer: string;
  /** One-line summary (may be empty). */
  summary: string;
  /** Longer markdown description (may be empty). */
  description: string;
  /** The API route the model serves, e.g. `/chat/completions`. */
  endpoint: string;
  /** Per-token pricing, absent for image/time-billed models. */
  pricing: ModelPricing | null;
  inputs: string[];
  outputs: string[];
  /** The model's context window in tokens, when the catalog reports it. */
  context_length: number | null;
  /** The model's maximum reply size in tokens, when the catalog reports it. */
  max_output_tokens: number | null;
  /** The model's release date (`YYYY-MM-DD`), when the catalog reports it. */
  released_at: string | null;
}

// ---- local models ----------------------------------------------------------

export type Accelerator = "metal" | "cuda" | "cpu";

/** The machine's compute profile, for hardware-aware model recommendations. */
export interface HardwareProfile {
  ram_bytes: number;
  vram_bytes: number | null;
  accelerator: Accelerator;
  chip_label: string;
  /** Bytes we plan against (pool minus OS/app headroom). */
  usable_budget: number;
}

export type RuntimeSource = "managed" | "system" | "none";

/** Status of the self-managed llama.cpp runtime. */
export interface RuntimeStatus {
  binary: string | null;
  source: RuntimeSource;
  managed_version: string;
  can_manage: boolean;
}

/** Streamed progress while installing the managed runtime (`runtime://install`). */
export type RuntimeInstallEvent =
  | { kind: "log"; line: string }
  | { kind: "progress"; downloaded: number; total: number | null };

/** How well a model is expected to run on this machine. */
export type Fit = "good" | "tight" | "too_big";

/** Where a model's weights are hosted. */
export type Origin =
  | { kind: "huggingface"; repo: string; file: string; revision: string }
  | { kind: "oxen"; repo: string; file: string; revision: string };

/** One downloadable GGUF at one quant. `id` is the on-disk name + served alias. */
export interface ModelRef {
  id: string;
  display: string;
  params: string;
  quant: string;
  context: number;
  size_bytes: number;
  origin: Origin;
}

/** A quant of a catalog model, annotated with fit + the exact ref to download. */
export interface QuantOption {
  quant: string;
  size_bytes: number;
  fit: Fit;
  installed: boolean;
  model: ModelRef;
}

/** A model offered in the setup wizard (a family with one or more quants). */
export interface CatalogModel {
  id: string;
  display: string;
  params: string;
  context: number;
  note: string;
  source: "curated" | "huggingface" | "oxen";
  quants: QuantOption[];
  recommended_quant: string | null;
  best_fit: Fit;
}

/** A Hugging Face search hit. */
export interface HfHit {
  repo: string;
  downloads: number;
  likes: number;
  params: string;
}

/** `local://status` payload — a phase of bringing a local model online, so the
 *  UI can show progress while switching to it (or while the active local
 *  model's server is restarted after it died). Nothing loads at launch. */
export interface LocalStatus {
  model: string;
  /** `"starting"` (runtime/GPU init), `"loading"` (reading weights), `"ready"`,
   *  or `"error"` (the load ended without a server). */
  phase: "starting" | "loading" | "ready" | "error";
}

/** Installed local models plus disk usage and runtime status. */
export interface InstalledView {
  models: ModelRef[];
  /** Bytes used by downloaded models. */
  total_disk_bytes: number;
  dir: string;
  runtime: RuntimeStatus;
  /** Total bytes on the volume holding the model store (null if unknown). */
  disk_total: number | null;
  /** Free bytes on that volume — used to warn before a download won't fit. */
  disk_free: number | null;
}

export interface DownloadProgress {
  id: string;
  downloaded: number;
  total: number | null;
  fraction: number | null;
}

// ---- themes ----------------------------------------------------------------

export interface ThemePalette {
  title: string;
  primary: string;
  secondary: string;
  text: string;
  muted: string;
  danger: string;
  link: string;
  background: string;
  surface: string;
  border: string;
}

export interface ThemeVoice {
  prompt_icon: string;
  prompt_label: string;
  spinner_glyphs: string[];
  thinking: string[];
  tool_verbs: Record<string, string[]>;
  deaths: string[];
  /** Big block-letter wordmark, e.g. "OXEN TRAIL". */
  wordmark: string;
  /** Small line above the wordmark, e.g. "～ The ～". */
  pre_tagline: string;
  /** One-line description under the wordmark. */
  subtitle: string;
  /** [label, value] rows rendered as the Oregon-Trail-style status panel. */
  flavor_top: [string, string][];
  flavor_bottom: [string, string][];
  /** "Press RETURN to size up the situation"-style hint under the hero. */
  bottom_hint: string;
  [key: string]: unknown;
}

/** Desktop typography + framing. The store maps these onto CSS tokens. */
export interface ThemeStyle {
  font_display: string;
  font_body: string;
  font_mono: string;
  display_transform: string; // "uppercase" | "none"
  display_spacing: string; // letter-spacing
  radius: string; // CSS length
  border_width: string; // CSS length
  shadow: string; // "pixel" | "soft" | "glow" | "none"
  hero: string; // "pixel" | "newspaper" | "minimal"
  scene: string; // "trail" | "grid" | "none" — the pixel hero's fallback artwork
  game?: string; // "tumbleweed" | "oregon" | "hunt" | "study" | "none" — the pixel hero's default game
  // (the player can switch cabinets at runtime; "none" opts into a static scene)
}

export interface Theme {
  meta: { name: string; author: string; description: string };
  palette: ThemePalette;
  voice: ThemeVoice;
  style: ThemeStyle;
}

export interface ThemeSummary {
  name: string;
  slug: string;
  description: string;
  builtin: boolean;
  installed: boolean;
  active: boolean;
}

// ---- clarifying questions --------------------------------------------------

export interface Choice {
  label: string;
  description: string;
}

export interface Question {
  question: string;
  header: string;
  options: Choice[];
  multiSelect: boolean;
}

export interface QuestionPayload {
  /** The chat that asked — a background chat's question lights its tab, not
   *  the visible chat's card. */
  session: string;
  id: string;
  questions: Question[];
}

export interface QuestionAnswer {
  header: string;
  question: string;
  selected: string[];
}

/** `agent://approval-request` payload — a gated tool call waiting on the
 *  user's decision (the permission gate's approval card). */
export interface ApprovalRequestEvent {
  session: string;
  id: string;
  kind: "shell" | "file_edit" | "git_commit" | "task_kill" | "ship";
  tool: string;
  command: string;
  risk: string;
  reasons: string[];
  grant_label: string;
  offer_project_grant: boolean;
  offer_trash: boolean;
}

/** `agent://approval` payload — a pending/resolved marker for the thread. */
export interface ApprovalEvent {
  session: string;
  phase: "pending" | "resolved";
  name: string;
  command: string;
  decision: string;
}

/** The decision sent back through `answer_approval`. */
export type ApprovalChoice = "once" | "session" | "project" | "trash" | "bypass" | "deny";

// ---- permissions settings ---------------------------------------------------

export type PermissionScope = "global" | "project";
export type PermissionRuleKind = "allow" | "allow_exact" | "deny";

/** One scope's saved permission rules (Settings → Permissions). */
export interface PermissionRuleSet {
  mode: string | null;
  allow: string[];
  allow_exact: string[];
  deny: string[];
}

/** How eagerly the permission gate asks before a tool runs. `bypass` is the
 *  CLI's `--yolo`: nothing asks, though circuit breakers still refuse. */
export type PermissionMode = "relaxed" | "cautious" | "bypass";

/** Everything the Permissions settings page renders. */
export interface PermissionsView {
  /** The effective default mode: "relaxed" | "cautious" | "bypass". */
  mode: string;
  global: PermissionRuleSet;
  project: PermissionRuleSet;
  project_path: string;
}

// ---- streamed agent events -------------------------------------------------

/** `agent://token` payload — a streamed token tagged with its chat session. */
export interface TokenEvent {
  session: string;
  token: string;
}

/** `agent://tool` payload, tagged with the chat session it belongs to. */
export interface ToolEvent {
  session: string;
  /** The model's id for the call; calls in one reply may run concurrently,
   *  so pair an `end` with its `start` by this id, not by name. */
  call_id?: string;
  phase: "start" | "end";
  name: string;
  detail: string;
}

/** `agent://tool-progress` payload — a chunk of a running tool's output (a
 *  shell command streaming), keyed by the call it belongs to. */
export interface ToolProgressEvent {
  session: string;
  call_id: string;
  name: string;
  chunk: string;
}

/** `agent://tool-delta` payload — a fragment of a tool call's JSON arguments,
 *  tagged with the tool name, streamed so the UI can show content as it's
 *  written (a file, a canvas document). */
export interface ToolDeltaEvent {
  session: string;
  name: string;
  delta: string;
}

/** `agent://usage` payload — a session's live usage, emitted around each model
 *  call within a turn so the meter tracks consumption as it accrues. */
export interface UsageEvent {
  session: string;
  tokens_used: number;
  context_tokens: number;
  context_window: number;
  prompt_tokens_used: number;
  completion_tokens_used: number;
}

/** `agent://compacted` payload — the transcript was trimmed to fit the context
 *  window, with a short note to show in the thread. */
export interface CompactedEvent {
  session: string;
  detail: string;
}

/** `agent://notice` payload — a one-line notice about something the agent did
 *  on its own; `kind` is `background_task` when a finished task's output was
 *  delivered to the model. */
export interface NoticeEvent {
  session: string;
  kind: string;
  text: string;
}

/** `agent://retry` payload — a model call hit a transient provider/network
 *  error and is being retried with backoff; shown as a thread notice so the
 *  pause reads as a hiccup (with the error for debugging), not a hang. */
export interface RetryEvent {
  session: string;
  /** Which attempt just failed (1-based). */
  attempt: number;
  max_attempts: number;
  /** How long the agent waits before the next attempt. */
  delay_ms: number;
  /** The one-line reason, as shown in the notice. */
  error: string;
  /** Set when the attempts on the current model are spent and the call moves to
   *  a configured fallback instead of failing the turn. The session model is
   *  unchanged — only this call switches. */
  switching_to?: string;
  /** The model whose call failed and the endpoint it was sent to. Empty from a
   *  server that predates the detail fields. */
  model?: string;
  endpoint?: string;
  /** The HTTP status of the failed reply, when there was one. */
  status?: number;
  /** What the provider actually said — the raw error body, or the transport
   *  error chain — for the error-detail view. Absent when `error` says it all. */
  detail?: string;
}

/** Everything the UI knows about one failed model call, kept on the thread
 *  notice (and the final "continue" card) so the faint one-liner can open into
 *  a view that actually helps debug: where it went, what came back, and what
 *  the agent did next. */
export interface ModelErrorDetail {
  /** When the failure was seen, epoch milliseconds. */
  at: number;
  /** The one-line reason (an `LlmError`'s display form). */
  error: string;
  model?: string;
  endpoint?: string;
  status?: number;
  /** The raw provider response / transport error chain, when there was one. */
  detail?: string;
  /** Which attempt failed (1-based) out of how many the policy allows. */
  attempt?: number;
  maxAttempts?: number;
  /** What happened next: a timed retry, or a switch to a fallback model. */
  next?: { kind: "retry"; delayMs: number } | { kind: "switch"; model: string };
}

/** One stream rule as stored in rules.json — a pattern that watches the
 *  model's output and a correction to send when it matches. */
export interface RuleSpec {
  name: string;
  /** The regular expression to watch for (the agent's engine, not JS's). */
  when: string;
  /** Where it watches: "text" (prose), "tool" (tool-call arguments), or both
   *  when empty. */
  scope: string[];
  message: string;
  /** Whether a match throws away the reply in flight and asks again. */
  interrupt: boolean;
  /** "once" (default) or "after:<n>" rounds. */
  repeat: string | null;
  enabled: boolean;
  /** What was asked for, on a rule an earlier version drafted with the model.
   *  Nothing reads it now; it round-trips so saving doesn't strip the file. */
  prompt?: string | null;
  /** A line this rule is meant to catch, which seeds the editor's tester. */
  sample?: string | null;
}

/** A rule worth offering, with the words needed to decide on it. */
export interface RuleSuggestion {
  /** What it does, in plain language. */
  title: string;
  /** Why you'd want it, in one line. */
  why: string;
  /** What it catches, as prose rather than the regex. */
  catches: string;
  /** Which family it belongs to ("Any project", "Rust", …). */
  group: string;
  rule: RuleSpec;
}

/** Both sets of rules for the active project. */
export interface RuleSets {
  /** The user's own, editable here. */
  user: RuleSpec[];
  /** Committed to the repository — shown, not edited. */
  project: RuleSpec[];
  project_path: string;
}

/** What a pattern does against a sample, checked by the agent's regex engine
 *  so the editor can't bless a pattern the engine would reject. */
export interface PatternCheck {
  error: string | null;
  /** Byte ranges of each match, in order. */
  matches: [number, number][];
}

/** The context-compression setting: off (send requests as recorded), audit
 *  (measure would-be savings without changing anything), or on (compress stale
 *  tool output before each request; originals stay retrievable). */
export type CompressionMode = "off" | "audit" | "on";

/** `agent://compression` payload — compression shrank ("on") or measured
 *  ("audit") a model call's request. Fires per model call within a turn, so the
 *  UI updates counters rather than appending thread notices. */
export interface CompressionEvent {
  session: string;
  mode: CompressionMode;
  /** Estimated tokens this model call saved (or would have saved). */
  saved_tokens: number;
  /** Cumulative estimated tokens saved across the session's run. */
  total_saved_tokens: number;
  /** How many tool results were compressed for this call. */
  results_compressed: number;
}

export type Mode = "light" | "dark";

/** A chat's run state for the sidebar indicator. Absent = idle / read. */
export type RunStatus = "running" | "unread";

// ---- settings (unified full-screen settings shell) -------------------------

/** The subpages of the full-screen Settings surface, used as the sidebar nav
 *  key and the deep-link target for `openSettings(page)`. */
export type SettingsPage =
  | "connection"
  | "cloud-models"
  | "local-models"
  | "tools"
  | "permissions"
  | "skills"
  | "preview"
  | "media"
  | "code-review"
  | "compression"
  | "usage"
  | "appearance"
  | "logs"
  | "rules";

/** Persisted live-preview preferences (mirrors `harness_runtime::preview`). */
export interface PreviewPrefs {
  auto_verify: boolean;
}

/** One model's accumulated usage, mirroring `ModelUsageRow` from the backend —
 *  the model id, its prompt/completion token totals, and the dollars spent. */
export interface ModelUsageRow {
  model: string;
  source: "oxen_cloud" | "unpriced";
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd: number | null;
}

/** The per-model usage breakdown (most-spent first) plus the grand total, for
 *  the Usage settings page. Mirrors `UsageBreakdown` from the backend. */
export interface UsageBreakdown {
  rows: ModelUsageRow[];
  total_cost_usd: number | null;
  prompt_tokens: number;
  completion_tokens: number;
  has_unpriced_usage: boolean;
}

/** One local-calendar day in the yearly usage activity grid. */
export interface DailyUsageRow {
  date: string;
  prompt_tokens: number;
  completion_tokens: number;
}

// ---- code review (the configurable find → verify → report pipeline) --------

/** One parallel reviewer within a fan-out step. Mirrors
 *  `harness_review::StepAgent`. */
export interface CodeReviewStepAgent {
  name: string;
  prompt: string;
}

/** One step of the code-review pipeline: a short name and either a single
 *  prompt or a set of parallel `agents` (a fan-out). Mirrors
 *  `harness_review::ReviewStep`. Templates may use `{{target}}`, `{{diff}}`,
 *  `{{previous}}` (the prior step's output), and `{{max_findings}}`. */
export interface CodeReviewStep {
  name: string;
  prompt: string;
  agents?: CodeReviewStepAgent[];
}

/** The saved pipeline (`~/.oxen-harness/code-review.json`), shared with the
 *  CLI's `/code-review`. Mirrors `harness_review::ReviewConfig`. */
export interface CodeReviewConfig {
  steps: CodeReviewStep[];
  max_findings: number;
  /** Cap on subagents running at once within a fan-out step. */
  max_parallel: number;
}

/** What `run_code_review` resolves with. On `"ok"` the user/assistant exchange
 *  is already persisted to the session; the UI appends it to the thread. */
export interface CodeReviewRunResult {
  status: "ok" | "nothing" | "cancelled";
  user: string;
  assistant: string;
  findings: number;
  /** Estimated tokens spent across every reviewer agent in the pipeline. */
  tokens_used: number;
}

/** `review://progress` — which pipeline step a running review is on. More than
 *  one entry in `agents` means the step fans out (a fleet panel opens too). */
export interface CodeReviewProgressEvent {
  session: string;
  step: string;
  index: number;
  total: number;
  agents: string[];
}

/** One subagent lane of a chat, running or finished (the agents hub). */
export interface AgentSummary {
  parent?: string;
  depth?: number;
  model?: string;
  stop?: string | null;
  has_patch?: boolean;
  /** The lane's id (its session id). */
  id: string;
  label: string;
  fleet: string;
  /** `running`, `done`, `partial`, `failed`, or `unknown`. */
  status: string;
  /** The reply's head, or the error, once finished. */
  summary: string;
  tokens: number;
  rounds: number;
  elapsed_secs: number;
  created_at: number;
}

/** One background shell task of a chat (`tasks://changed`, `list_tasks`). */
export interface TaskSummary {
  id: number;
  command: string;
  running: boolean;
  exit_code: number | null;
  killed: boolean;
  elapsed_secs: number;
  last_line: string;
}

/** `tasks://changed` — the whole list of a chat's background tasks after
 *  one started, ended, or was killed. */
export interface TasksChangedEvent {
  session: string;
  tasks: TaskSummary[];
}

// ---- media generation (generate_image / generate_video) --------------------

/** One image/video generation of a project (`media://changed`, `list_media`).
 *  Paths are project-relative. Mirrors `harness_protocol::MediaItem`. */
export interface MediaItem {
  id: string;
  session: string;
  /** The turn within that chat: the persisted seq of the user message that
   *  started it. Null on rows recorded before it was tracked. */
  turn_seq: number | null;
  /** The model's id for the tool call that made it. */
  call_id: string | null;
  batch: string;
  index: number;
  kind: "image" | "video";
  model: string;
  prompt: string;
  params: Record<string, unknown>;
  refs: string[];
  path: string | null;
  poster: string | null;
  bytes: number;
  width: number | null;
  height: number | null;
  duration_secs: number | null;
  cost_usd: number | null;
  status: "queued" | "processing" | "succeeded" | "failed" | "cancelled" | "timed_out";
  error: string | null;
  created_at: number;
  completed_at: number | null;
  parent: string | null;
  seed: unknown;
  /** Where each reference came from, in request order (the provenance
   *  behind `refs`). Empty on rows recorded before it was tracked. */
  sources: MediaSource[];
  /** The prompt as the agent wrote it, when rewriting reference labels
   *  changed what the hub got (`prompt`). */
  agent_prompt: string | null;
  /** The hub's completed generation record, verbatim. */
  provider: unknown;
}

/** One reference's provenance: the stored copy, the file it was made from,
 *  and how that file got into the request. */
export interface MediaSource {
  /** The project-relative copy under `refs/` (the matching `refs` entry). */
  path: string;
  origin: "attachment" | "generation" | "file";
  /** The chip label the request used (`[Image #1]`), when it came from the chat. */
  label: string | null;
  /** The original file: absolute for an attachment from outside the project,
   *  project-relative otherwise. */
  source: string;
  /** The library item whose output this is, for a `generation` origin. */
  generation: string | null;
  kind: string;
  sha256: string;
}

/** `media://changed` — the whole media library of a project after a
 *  generation was queued, progressed, finished, or failed. */
export interface MediaChangedEvent {
  session: string;
  root: string;
  items: MediaItem[];
  /** Reference uploads in flight for this project (live-only; absent on
   *  older payloads and after a generation is recorded). */
  uploads?: MediaUpload[];
}

/** One reference file on its way to the hub (`media://changed` `uploads`). */
export interface MediaUpload {
  id: string;
  /** The chat that triggered it. */
  session: string;
  /** The chip label (`[Image #1]`) when it came from an attachment. */
  label: string | null;
  filename: string;
  /** `image`, `video`, or `audio`. */
  kind: string;
  bytes_sent: number;
  bytes_total: number;
  /** `uploading`, `presigning`, `done`, `reused`, or `failed`. */
  status: string;
  error: string | null;
  /** Unix seconds. */
  started_at: number;
}

/** Persisted media-generation preferences (mirrors `harness_media::MediaPrefs`).
 *  A `null` budget means "always ask". */
export interface MediaPrefs {
  default_image_model: string;
  default_video_model: string;
  output_dir: string;
  per_generation_usd: number | null;
  per_run_usd: number | null;
  /** `namespace/repo` on hub.oxen.ai where the hub also keeps every
   *  generation (under the output folder); null = files stay in the project. */
  hub_repo: string | null;
  /** When the project is an Oxen repo, add and commit each finished batch. */
  commit_with_oxen: boolean;
}

/** One image/video model of the hub catalog, for pickers. */
export interface MediaModelSummary {
  id: string;
  kind: "image" | "video";
  price: string;
  developer: string | null;
  summary: string | null;
  inputs: string[];
}

// ---- fleets (N parallel subagents: review fan-out or spawn_agents) ----------

/** `fleet://started` — a fleet of parallel subagents is spinning up. `fleet`
 *  names it on every later event and on the stop request: a background
 *  (`wait: false`) fleet can overlap another in the same chat. */
export interface FleetStartedEvent {
  session: string;
  fleet: string;
  agents: string[];
  /** `"review"` (a pipeline step) or `"turn"` (the model's spawn_agents). */
  source: "review" | "turn";
  /** The model's id for the tool call that spawned the fleet, when one did:
   *  the thread places the lanes beside that call. */
  call?: string;
}

/** `fleet://agent` — one lane changed state. `lane` is the lane's own id,
 *  what a per-lane stop or steer takes. */
export interface FleetAgentEvent {
  session: string;
  fleet: string;
  agent: number;
  lane: string;
  name: string;
  phase: "started" | "done" | "failed" | "partial" | "cancelled";
  tokens: number;
  summary: string;
}

/** `fleet://budget` — where the turn's tree budget stands (every lane of a
 *  root turn spends from one wallet), sent whenever a lane's spend changes. */
export interface FleetBudgetEvent {
  session: string;
  fleet: string;
  tokens: number;
  max_tokens: number;
  requests: number;
  max_requests: number;
  spawns: number;
  max_spawns: number;
}

/** `fleet://agent-activity` — what one lane is doing right now. */
export interface FleetActivityEvent {
  session: string;
  fleet: string;
  agent: number;
  /** `note`: a one-line notice (a refused command, a nudge, a retry). */
  kind: "token" | "tool" | "tokens" | "note";
  text: string;
  tokens: number | null;
}

/** `review://token` — streamed text from the current review step's agent. */
export interface CodeReviewTokenEvent {
  session: string;
  token: string;
}

/** `review://tool` — a tool the current review step's agent invoked. */
export interface CodeReviewToolEvent {
  session: string;
  name: string;
}

/** One agent tool (built-in or custom) as shown on the Tools page: its identity,
 *  the (possibly overridden) description advertised to the model, its JSON schema,
 *  and whether the user has it enabled. Mirrors `harness_runtime::tools::ToolInfo`. */
export interface ToolInfo {
  /** Stable tool id the model calls (e.g. `read_file`). */
  name: string;
  /** Description currently advertised to the model (override if set, else default). */
  description: string;
  /** The tool's built-in default description, shown when an override is active. */
  default_description: string;
  /** JSON Schema for the tool's arguments. */
  parameters: unknown;
  /** Whether the tool is registered for new agents. */
  enabled: boolean;
  /** True for the always-on core tools the harness ships. */
  builtin: boolean;
  /** Free-form per-tool config (e.g. shell timeout), as a JSON object. */
  config: Record<string, unknown>;
}

/** Where a skill lives. Mirrors `harness_tools::SkillScope`. */
export type SkillScope = "global" | "project";

/** One skill as shown on the Skills settings page: a SKILL.md the model can
 *  load on demand. Mirrors `harness_runtime::skills::SkillInfo`. */
export interface SkillInfo {
  /** Stable identifier the model passes to the `skill` tool (the directory name). */
  name: string;
  /** One-line "when to use this" trigger, advertised to the model. */
  description: string;
  /** The full SKILL.md body — the instructions loaded on invocation. */
  instructions: string;
  /** global = every project; project = travels with this repository. */
  scope: SkillScope;
  /** The skill's directory on disk, for supporting files. */
  dir: string;
  /** Whether the skill is offered to the model. */
  enabled: boolean;
}

/** A user-defined tool backed by a simple external action. Mirrors
 *  `harness_tools::CustomToolSpec`. */
export interface CustomToolSpec {
  /** Tool id the model calls — lowercase letters, digits, underscores. */
  name: string;
  /** Tells the model what the tool does and when to reach for it. */
  description: string;
  /** JSON Schema object describing the tool's arguments. */
  parameters: unknown;
  /** What invoking the tool does. Only HTTP POST today. */
  action: { kind: "http_post"; url: string };
}

// ---- canvas (side-panel documents) -----------------------------------------

export type CanvasFormat = "markdown" | "html" | "code" | "svg";

/** A document the agent showed in the side-panel canvas. Addressed by `id` so a
 *  later update with the same id replaces it. */
export interface CanvasDoc {
  id: string;
  title: string;
  format: CanvasFormat;
  language?: string | null;
  content: string;
  /** The workspace-relative file the document mirrors, when the agent showed
   *  a project file: the panel re-reads it whenever the file changes. */
  path?: string | null;
}

/** `agent://canvas` payload — a CanvasDoc tagged with its chat session. */
export interface CanvasEvent extends CanvasDoc {
  session: string;
}

/** The `agent://open-file` payload: the model's `open_file` tool putting
 *  workspace-relative project files into the Editor/viewer dock. */
export interface OpenFileEvent {
  session: string;
  paths: string[];
}

/** The `fs://changed` payload: one debounced batch of on-disk changes under a
 *  watched workspace root (workspace-relative paths). Empty `paths` means
 *  "too much changed to enumerate — refresh everything you have loaded". */
export interface FsChangedEvent {
  root: string;
  paths: string[];
}

// ---- live preview (dev servers) ---------------------------------------------

export type PreviewPhase = "starting" | "ready" | "error" | "stopped";

/** A dev server's lifecycle snapshot (mirrors `harness_preview::PreviewStatus`). */
export interface PreviewStatus {
  phase: PreviewPhase;
  /** Short server name, e.g. "dev". */
  name: string;
  /** The shell command the server was started with. */
  command: string;
  /** Loadable URL, once known (always set when phase is "ready"). */
  url: string | null;
  port: number | null;
  /** Human-readable detail for error/stopped phases. */
  message: string | null;
}

/** `preview://status` payload — a PreviewStatus tagged with its chat session. */
export interface PreviewEvent extends PreviewStatus {
  session: string;
}

/** `preview://console` payload — the preview page hit a JavaScript error. */
export interface PreviewConsoleEvent {
  session: string;
  text: string;
}

/** The preview placeholder's rectangle, in CSS pixels. */
export interface PreviewBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

// ---- plan (task checklist) -------------------------------------------------

export type PlanStatus = "pending" | "in_progress" | "completed";

/** One item in the agent's task plan, from an `update_plan` tool call. Mirrors
 *  `harness_tools::plan::PlanItem`. */
export interface PlanItem {
  /** Imperative description, e.g. "Wire CLI rendering". */
  content: string;
  /** Present-continuous form shown while active, e.g. "Wiring CLI rendering". */
  active_form: string;
  status: PlanStatus;
}

// ---- workspace files (the Files tree + Editor/viewer dock) ------------------

/** One row in the Files tree. */
export interface FileEntry {
  name: string;
  /** Workspace-relative path (`/`-joined) — the tree's stable key. */
  path: string;
  is_dir: boolean;
}

/** A text file's content for the editor. */
export interface FileBody {
  content: string;
  /** True when the file was longer than the backend's read cap — the editor
   *  opens read-only so a save can't destroy the unread tail. */
  truncated: boolean;
  size: number;
}

/** VS Code-style summary of one changed file in the workspace's git tree. */
export type GitStatusKind =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "conflicted";

/** A changed path in the workspace's Git working tree. */
export interface GitFileState {
  /** Workspace-relative path (`/`-joined) — matches the Files tree's keys. */
  path: string;
  /** Where a rename came from, when Git reports one. */
  original_path: string | null;
  status: GitStatusKind;
  /** Git's index status letter (space when clean). */
  index: string;
  /** Git's working-tree status letter (space when clean). */
  worktree: string;
}

/** A unified diff for one changed file. */
export interface GitFileDiff {
  content: string;
  /** True when the diff was longer than the backend's read cap. */
  truncated: boolean;
}

/** A highlighted code selection staged as context for the next prompt. */
export interface CodeSnippet {
  /** Workspace-relative path of the file the selection came from. */
  path: string;
  /** 1-based first and last line of the selection. */
  start: number;
  end: number;
  code: string;
}

/** `project://open` — a directory (and optionally a surface to open there)
 *  arrived from the command line while the app was running. */
export interface ProjectOpenEvent {
  path: string;
  /** `gallery`, `settings:<page>`, or `session:<id>`. */
  surface: string | null;
}

// ---- the codebase study game ("Trail of Understanding") ----------------------
// Mirrors harness_protocol's Study* DTOs.

/** One territory of the project (a crate, a feature directory, the docs)
 *  with the player's decayed mastery of it. */
export interface StudyTerritory {
  /** The territory's path relative to the workspace root (`docs` for root Markdown). */
  id: string;
  name: string;
  /** 0..1, after spaced-repetition decay. */
  mastery: number;
  answered: number;
  correct: number;
  last_answered_at?: number;
  /** How far mastery has fallen from its peak, 0..1. */
  faded: number;
  /** Source files in the region; bigger regions take more answers. */
  files?: number;
  /** Questions here whose spaced review has come due. */
  due?: number;
}

export interface StudyProfile {
  project: string;
  workspace: string;
  /** 0..100 across the whole project. */
  understanding: number;
  level: number;
  answered: number;
  /** Reviews due right now, across regions. */
  due?: number;
  territories: StudyTerritory[];
}

export type StudyMode = "expedition" | "fresh_tracks" | "ride_along" | "review";
export type StudyVerdict = "full" | "partial" | "wrong";

/** A question as the client sees it — the answer stays server-side. */
export interface StudyQuestion {
  id: string;
  territory: string;
  /** `order`: put the options in sequence, answered with their numbers ("3142"). */
  kind: "multiple_choice" | "true_false" | "free_text" | "order";
  prompt: string;
  options: string[];
  source_path: string;
  source_lines?: [number, number];
  source_excerpt: string;
  difficulty: number;
  cached: boolean;
}

export interface StudyBatchRequest {
  mode: StudyMode;
  count?: number;
  /** Question ids already used this run. */
  exclude?: string[];
}

export interface StudyBatch {
  mode: string;
  questions: StudyQuestion[];
  territory: string;
  tokens_used: number;
  model: string;
}

export interface StudyAnswerRequest {
  question_id: string;
  /** The option index (as text) for choice questions, the option numbers in
   *  sequence for an ordering, the typed answer otherwise. */
  answer: string;
  hint_used?: boolean;
}

export interface StudyGrade {
  verdict: StudyVerdict;
  feedback: string;
  correct_answer: string;
  explanation: string;
}

export interface StudyAnswerResult {
  grade: StudyGrade;
  profile: StudyProfile;
  tokens_used: number;
}

/** Per-role model overrides from limits.json; unset roles fall back
 *  (study → smol → the session model). */
export interface ModelRoles {
  study?: string | null;
  smol?: string | null;
  summary?: string | null;
}

export type ModelRole = "study" | "smol" | "summary";
