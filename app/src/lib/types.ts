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

/** A project = a working directory the agent runs in. Chats are grouped by it. */
export interface Project {
  path: string;
  name: string;
  session_count: number;
  active: boolean;
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

/** A cloud model in the catalog. `id` is sent to the inference API; `name` is a
 *  friendly label. `builtin` models can't be removed; `selected` is the current
 *  default. */
export interface CloudModel {
  id: string;
  name: string;
  builtin: boolean;
  selected: boolean;
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
 *  UI can show progress while switching to it. */
export interface LocalStatus {
  model: string;
  /** `"starting"` (runtime/GPU init), `"loading"` (reading weights), `"ready"`. */
  phase: "starting" | "loading" | "ready";
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
  scene: string; // "trail" | "grid" | "none" — the pixel hero's artwork
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
}

/** `agent://compacted` payload — the transcript was trimmed to fit the context
 *  window, with a short note to show in the thread. */
export interface CompactedEvent {
  session: string;
  detail: string;
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
  | "appearance"
  | "logs";

/** One built-in agent tool as shown on the Tools page: its identity, the
 *  (possibly overridden) description advertised to the model, its JSON schema,
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
