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
}

export interface SessionView {
  info: SessionInfo;
  messages: ChatMessage[];
  /** True when the chat is mid-turn and couldn't be read; keep the live thread. */
  running: boolean;
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
  session_count: number;
  active: boolean;
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
 *  UI can show progress while switching to it (or while a restored selection
 *  starts lazily after an app relaunch). */
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
  game?: string; // "tumbleweed" | "oregon" | "none" — the pixel hero's default game
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
  id: string;
  questions: Question[];
}

export interface QuestionAnswer {
  header: string;
  question: string;
  selected: string[];
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
  phase: "start" | "end";
  name: string;
  detail: string;
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
  error: string;
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
  | "skills"
  | "preview"
  | "code-review"
  | "compression"
  | "usage"
  | "appearance"
  | "logs";

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

// ---- fleets (N parallel subagents: review fan-out or spawn_agents) ----------

/** `fleet://started` — a fleet of parallel subagents is spinning up. */
export interface FleetStartedEvent {
  session: string;
  agents: string[];
  /** `"review"` (a pipeline step) or `"turn"` (the model's spawn_agents). */
  source: "review" | "turn";
}

/** `fleet://agent` — one lane changed state. */
export interface FleetAgentEvent {
  session: string;
  agent: number;
  name: string;
  phase: "started" | "done" | "failed";
  tokens: number;
  summary: string;
}

/** `fleet://agent-activity` — what one lane is doing right now. */
export interface FleetActivityEvent {
  session: string;
  agent: number;
  kind: "token" | "tool" | "tokens";
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
}

/** `agent://canvas` payload — a CanvasDoc tagged with its chat session. */
export interface CanvasEvent extends CanvasDoc {
  session: string;
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

// ---- verification loops ---------------------------------------------------

export interface LoopSummary {
  name: string;
  slug: string;
  description: string;
  verify: string;
  builtin: boolean;
  installed: boolean;
}

export interface LoopSpec {
  schema_version: number;
  name: string;
  description: string;
  goal: string;
  success_criteria: string[];
  verify?: { type: "command"; command: string; timeout_ms: number } | { type: "rubric"; threshold: number } | null;
  gates: unknown[];
  max_iterations: number;
  token_budget?: number | null;
}

export interface LoopRunResult {
  succeeded: boolean;
  iterations: number;
  summary: string;
}
