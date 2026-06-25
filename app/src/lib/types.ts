// Wire types — kept in sync with the Tauri commands in src-tauri/src/lib.rs and
// the Rust crates they return. Field names match Rust serde output (snake_case
// unless a struct renames, e.g. `multiSelect`).

export interface SessionInfo {
  model: string;
  workspace: string;
  session_id: string;
}

export interface SessionSummary {
  id: string;
  workspace: string;
  model: string;
  created_at: number;
  title: string | null;
  message_count: number;
}

/** A function/tool call inside an assistant message (OpenAI tool-calling shape). */
export interface ToolCall {
  id: string;
  type: string;
  function: { name: string; arguments: string };
}

/** One part of a multimodal message (text, image, or file). Matches the Rust
 *  `ContentPart` (untagged) — only text parts carry displayable text. */
export interface ContentPart {
  type: string;
  text?: string;
}

/** A message's content: a plain string, or — for messages with attachments —
 *  an array of parts (the Rust `MessageContent` enum serializes untagged). */
export type MessageContent = string | ContentPart[];

/** One stored transcript message (harness-llm ChatMessage). */
export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: MessageContent | null;
  tool_calls?: ToolCall[] | null;
  tool_call_id?: string | null;
  name?: string | null;
}

export interface SessionView {
  info: SessionInfo;
  messages: ChatMessage[];
  /** True when the chat is mid-turn and couldn't be read; keep the live thread. */
  running: boolean;
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

// ---- local models ----------------------------------------------------------

export interface ModelStatus {
  id: string;
  display: string;
  params: string;
  quant: string;
  context: number;
  note: string;
  installed: boolean;
  size_bytes: number;
  size_is_actual: boolean;
}

export interface ModelsView {
  models: ModelStatus[];
  total_disk_bytes: number;
  dir: string;
  llama_installed: boolean;
  can_auto_install: boolean;
  install_hint: string;
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

export type Mode = "light" | "dark";

/** A chat's run state for the sidebar indicator. Absent = idle / read. */
export type RunStatus = "running" | "unread";

// ---- canvas (side-panel documents) -----------------------------------------

export type CanvasFormat = "markdown" | "html" | "code" | "svg" | "mermaid";

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
