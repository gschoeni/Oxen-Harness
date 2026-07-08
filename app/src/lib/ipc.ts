// Typed wrappers over Tauri commands and events. Components import from here —
// never call `invoke`/`listen` directly.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  CatalogModel,
  CloudModel,
  ConnectionView,
  DownloadProgress,
  HardwareProfile,
  HfHit,
  InstalledView,
  LocalStatus,
  ModelRef,
  RuntimeInstallEvent,
  RuntimeStatus,
  QuestionAnswer,
  QuestionPayload,
  SessionInfo,
  SessionSummary,
  CanvasEvent,
  Project,
  SessionView,
  Theme,
  ThemeSummary,
  TokenEvent,
  ToolEvent,
  ToolDeltaEvent,
  UsageEvent,
  CompactedEvent,
  CompressionEvent,
  CompressionMode,
  RetryEvent,
  ChatMessage,
  ToolDefinition,
  ToolInfo,
  CustomToolSpec,
  SkillInfo,
  SkillScope,
  CodeReviewConfig,
  CodeReviewProgressEvent,
  CodeReviewRunResult,
  CodeReviewTokenEvent,
  CodeReviewToolEvent,
} from "./types";

// ---- session / agent -------------------------------------------------------

export const sessionInfo = () => invoke<SessionInfo>("session_info");
export const listSessions = () => invoke<SessionSummary[]>("list_sessions");
/** All-time total tokens used across every stored session (a running grand total). */
export const totalTokensUsed = () => invoke<number>("total_tokens_used");
/** A session's raw persisted transcript (verbatim, read-only) for the dev inspector. */
export const sessionMessages = (id: string) => invoke<ChatMessage[]>("session_messages", { id });
/** The tool definitions (JSON schemas) the current agent advertises to the model. */
export const toolDefinitions = () => invoke<ToolDefinition[]>("tool_definitions");

// ---- tools (manage which tools the agent may call) -------------------------

/** Every tool (built-in + custom) with its enabled/override state, for the Tools page. */
export const listTools = () => invoke<ToolInfo[]>("list_tools");
/** Add a new custom HTTP tool, or update the one with the same name. */
export const addCustomTool = (spec: CustomToolSpec) => invoke<void>("add_custom_tool", { spec });
/** Remove a custom tool (built-ins can only be disabled). */
export const removeCustomTool = (name: string) => invoke<void>("remove_custom_tool", { name });

// ---- skills (reusable SKILL.md instructions the model loads on demand) ------

/** Every skill visible from the active project, for the Skills page. */
export const listSkills = () => invoke<SkillInfo[]>("list_skills");
/** Create a skill, or update the one with the same name + scope. */
export const saveSkill = (scope: SkillScope, name: string, description: string, instructions: string) =>
  invoke<void>("save_skill", { scope, name, description, instructions });
/** Delete a skill's directory (SKILL.md plus supporting files). */
export const deleteSkill = (scope: SkillScope, name: string) =>
  invoke<void>("delete_skill", { scope, name });
/** Enable or disable a skill (applies to new/resumed chats). */
export const setSkillEnabled = (name: string, enabled: boolean) =>
  invoke<void>("set_skill_enabled", { name, enabled });
/** Enable or disable a built-in tool (applies to new/resumed chats). */
export const setToolEnabled = (name: string, enabled: boolean) =>
  invoke<void>("set_tool_enabled", { name, enabled });
/** Override (or clear, with null) the description the model sees for a tool. */
export const setToolDescription = (name: string, description: string | null) =>
  invoke<void>("set_tool_description", { name, description });

// ---- context compression (shrink stale tool output on the wire) -------------

/** The persisted context-compression mode ("off" | "audit" | "on"). */
export const getCompressionMode = () => invoke<CompressionMode>("get_compression_mode");
/** Set the context-compression mode: persisted for new chats and applied to
 *  the live conversation in place. Returns the refreshed session info. */
export const setCompressionMode = (mode: CompressionMode) =>
  invoke<SessionInfo>("set_compression_mode", { mode });
/** All-time tokens compression saved (or, in audit mode, would have saved). */
export const totalTokensSaved = () => invoke<number>("total_tokens_saved");

// ---- logs / fine-tuning export ---------------------------------------------

/** Write the given sessions to `path` as Oxen chat-completions fine-tuning JSONL;
 *  resolves with the number of conversations written. */
export const exportFinetuning = (path: string, sessionIds: string[], includeTools: boolean) =>
  invoke<number>("export_finetuning", { path, sessionIds, includeTools });

/** Open a native "save as" dialog for the export, defaulting to a `.jsonl` name.
 *  Returns the chosen path (or null if cancelled). */
export async function pickExportPath(defaultName = "finetuning.jsonl"): Promise<string | null> {
  const chosen = await save({
    title: "Export fine-tuning data",
    defaultPath: defaultName,
    filters: [{ name: "JSON Lines", extensions: ["jsonl"] }],
  });
  return chosen ?? null;
}
/** Load an attachment (absolute path, or relative to a session's workspace) as a
 *  data: URI for display — used by the composer preview and chat history. */
export const attachmentDataUri = (path: string, session?: string) =>
  invoke<string>("attachment_data_uri", { path, session });
export const newSession = () => invoke<SessionInfo>("new_session");
export const resumeSession = (id: string) => invoke<SessionView>("resume_session", { id });
/** Permanently delete a chat session and its messages. */
export const deleteSession = (id: string) => invoke<void>("delete_session", { id });

/** Set a chat's training-data review status ("" | "kept" | "rejected"). */
export const setReviewStatus = (id: string, status: string) =>
  invoke<void>("set_review_status", { id, status });

/** Bulk-set the review status for many chats at once; resolves with the count changed. */
export const setReviewStatusMany = (ids: string[], status: string) =>
  invoke<number>("set_review_status_many", { ids, status });

// ---- projects (chats grouped by working directory) -------------------------

export const listProjects = () => invoke<Project[]>("list_projects");
/** Add a folder as a project and make it active; new chats root there. */
export const openProject = (path: string) => invoke<Project>("open_project", { path });
/** Switch the active project to an already-known directory. */
export const setActiveProject = (path: string) => invoke<void>("set_active_project", { path });

/** Open a native folder picker, returning the chosen directory (or null). */
export async function pickFolder(): Promise<string | null> {
  const selected = await open({ directory: true, multiple: false, title: "Open a project folder" });
  return typeof selected === "string" ? selected : null;
}

/** Run one user turn in `session`; streams via session-tagged `agent://token` /
 *  `agent://tool` events. Resolves with the assistant's final text. Multiple
 *  sessions can run at once, so a chat keeps going after you switch away.
 *  `attachments` are absolute file paths of dropped images/PDFs to send along. */
export const runTurn = (session: string, prompt: string, attachments: string[] = []) =>
  invoke<string>("run_turn", { session, prompt, attachments });

/** Stop the in-flight turn in `session`, killing the model stream (local or
 *  remote). The `runTurn` promise then resolves with whatever streamed so far. */
export const cancelTurn = (session: string) => invoke<void>("cancel_turn", { session });

/** Retry a turn that failed before replying (e.g. a 401 before an API key was
 *  set), continuing the same conversation. The failed attempt's user message is
 *  already recorded, so this re-drives it without duplicating it. Resolves with
 *  the assistant's final text, exactly like `runTurn`. */
export const retryTurn = (session: string) => invoke<string>("retry_turn", { session });

/** Open a native file picker for images and PDFs, returning the chosen absolute
 *  paths (empty if the user cancels). These feed the same attachment flow as an
 *  OS file drop. */
export async function pickAttachments(): Promise<string[]> {
  const selected = await open({
    multiple: true,
    title: "Attach images or PDFs",
    filters: [
      { name: "Images & PDFs", extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "heic", "pdf"] },
    ],
  });
  if (!selected) return [];
  return Array.isArray(selected) ? selected : [selected];
}

/** Subscribe to OS file drops onto the window. Fires with the dropped absolute
 *  paths. Returns an unlisten function. */
export const onFileDrop = (handler: (paths: string[]) => void) =>
  getCurrentWebview().onDragDropEvent((e) => {
    if (e.payload.type === "drop") handler(e.payload.paths);
  });

export const onToken = (handler: (e: TokenEvent) => void) =>
  listen<TokenEvent>("agent://token", (e) => handler(e.payload));

export const onTool = (handler: (e: ToolEvent) => void) =>
  listen<ToolEvent>("agent://tool", (e) => handler(e.payload));

/** Streamed fragments of a tool call's JSON args (file/canvas content forming). */
export const onToolDelta = (handler: (e: ToolDeltaEvent) => void) =>
  listen<ToolDeltaEvent>("agent://tool-delta", (e) => handler(e.payload));

/** Fires at the end of each turn with the session's cumulative token count. */
export const onUsage = (handler: (e: UsageEvent) => void) =>
  listen<UsageEvent>("agent://usage", (e) => handler(e.payload));

/** Fires when the transcript was compacted mid-turn to fit the context window. */
export const onCompacted = (handler: (e: CompactedEvent) => void) =>
  listen<CompactedEvent>("agent://compacted", (e) => handler(e.payload));

/** Fires when compression shrank (or, in audit mode, measured) a model call's
 *  request — per model call, carrying the session's running savings. */
export const onCompression = (handler: (e: CompressionEvent) => void) =>
  listen<CompressionEvent>("agent://compression", (e) => handler(e.payload));

/** Fires when a model call hit a transient provider/network error and is being
 *  retried with backoff — so the thread can show the hiccup instead of hanging. */
export const onRetry = (handler: (e: RetryEvent) => void) =>
  listen<RetryEvent>("agent://retry", (e) => handler(e.payload));

export const onQuestion = (handler: (q: QuestionPayload) => void) =>
  listen<QuestionPayload>("agent://question", (e) => handler(e.payload));

export const onCanvas = (handler: (e: CanvasEvent) => void) =>
  listen<CanvasEvent>("agent://canvas", (e) => handler(e.payload));

/** Fires when the model starts writing a canvas, before its content arrives —
 *  so the panel can open in a "writing…" state. */
export const onCanvasWriting = (handler: (session: string) => void) =>
  listen<{ session: string }>("agent://canvas-writing", (e) => handler(e.payload.session));

export const answerQuestion = (id: string, answers: QuestionAnswer[]) =>
  invoke<void>("answer_question", { id, answers });

// ---- code review (the configurable find → verify → report pipeline) --------

/** Run the code-review pipeline in `session`'s workspace: uncommitted changes,
 *  or PR-style against `baseBranch`. Streams `review://progress` / `review://token`
 *  / `review://tool`; on success the findings are already injected into the
 *  session as a settled exchange. `cancelTurn(session)` stops it. */
export const runCodeReview = (session: string, baseBranch?: string) =>
  invoke<CodeReviewRunResult>("run_code_review", { session, baseBranch: baseBranch ?? null });

/** The saved code-review pipeline (steps + findings cap), for Settings. */
export const getCodeReviewConfig = () => invoke<CodeReviewConfig>("get_code_review_config");

/** Persist the pipeline; applies to the next review (CLI and desktop share it). */
export const saveCodeReviewConfig = (config: CodeReviewConfig) =>
  invoke<void>("save_code_review_config", { config });

/** The built-in default pipeline, for "reset to defaults". */
export const defaultCodeReviewConfig = () =>
  invoke<CodeReviewConfig>("default_code_review_config");

/** Fires when a running review moves to its next pipeline step. */
export const onCodeReviewProgress = (handler: (e: CodeReviewProgressEvent) => void) =>
  listen<CodeReviewProgressEvent>("review://progress", (e) => handler(e.payload));

/** Streamed text from the current review step's agent (live activity feed). */
export const onCodeReviewToken = (handler: (e: CodeReviewTokenEvent) => void) =>
  listen<CodeReviewTokenEvent>("review://token", (e) => handler(e.payload));

/** Fires when the current review step's agent invokes a tool. */
export const onCodeReviewTool = (handler: (e: CodeReviewToolEvent) => void) =>
  listen<CodeReviewToolEvent>("review://tool", (e) => handler(e.payload));

// ---- connection settings ---------------------------------------------------

/** Current Oxen API key + host overrides, with context for the Settings page. */
export const getConnection = () => invoke<ConnectionView>("get_connection");

/** Persist the Oxen API key + host (and Brave web-search key) and rebuild the
 *  agent. Blank fields fall back to env / CLI login. Resolves with the new
 *  session info. */
export const setConnection = (host: string, apiKey: string, braveApiKey: string) =>
  invoke<SessionInfo>("set_connection", { host, apiKey, braveApiKey });

/** Save just the Brave Search API key and apply it to the running agent without
 *  rebuilding it — so a failed web search can be retried in the same chat. */
export const configureBraveKey = (key: string) =>
  invoke<void>("configure_brave_key", { key });

/** Save the Oxen API key and authenticate `session`'s agent in place (no new
 *  session), so a turn that failed with a 401 can be retried inline in the same
 *  chat. Pair with `retryTurn` to re-drive the failed turn. */
export const configureOxenKey = (session: string, key: string) =>
  invoke<void>("configure_oxen_key", { session, key });

// ---- local models ----------------------------------------------------------

/** Installed local models, total disk used, and the runtime status. */
export const installedLocalModels = () => invoke<InstalledView>("installed_local_models");
/** The machine's compute profile, for hardware-aware recommendations. */
export const detectHardware = () => invoke<HardwareProfile>("detect_hardware");
/** Status of the self-managed llama.cpp runtime. */
export const runtimeStatus = () => invoke<RuntimeStatus>("runtime_status");
/** Download + set up the managed `llama-server` (streams `runtime://install`). */
export const installRuntime = () => invoke<void>("install_runtime");
/** The setup catalog: curated (fit + quant annotated) + featured Oxen models. */
export const listModelCatalog = () => invoke<CatalogModel[]>("list_model_catalog");
/** Resolve a pasted Hugging Face repo/GGUF link into an annotated model. */
export const resolveHfModel = (input: string) =>
  invoke<CatalogModel>("resolve_hf_model", { input });
/** Search the Hugging Face hub for GGUF repos. */
export const searchHfModels = (query: string) => invoke<HfHit[]>("search_hf_models", { query });
/** Whether a Hugging Face token is saved (for gated repos). */
export const hfTokenPresent = () => invoke<boolean>("hf_token_present");
/** Save (or clear) the Hugging Face token. */
export const setHfToken = (token: string) => invoke<void>("set_hf_token", { token });
/** Download a specific model (a chosen quant), streaming `models://progress`. */
export const downloadModel = (model: ModelRef) => invoke<void>("download_model", { model });
/** Delete a downloaded local model by id. */
export const removeModel = (id: string) => invoke<void>("remove_model", { id });
/** Switch the current session to a downloaded local model (starts a fresh chat). */
export const useLocalModel = (id: string) => invoke<SessionInfo>("use_local_model", { id });
/** Homebrew fallback when the platform has no managed runtime (`llama://install`). */
export const installLlama = () => invoke<void>("install_llama");

// ---- cloud models ----------------------------------------------------------

/** The cloud model catalog (built-ins + custom), with the selected one flagged. */
export const listCloudModels = () => invoke<CloudModel[]>("list_cloud_models");
/** Add (or rename) a custom cloud model; resolves with the updated catalog. */
export const addCloudModel = (id: string, name: string) =>
  invoke<CloudModel[]>("add_cloud_model", { id, name });
/** Remove a custom cloud model (built-ins can't be removed); returns the catalog. */
export const removeCloudModel = (id: string) =>
  invoke<CloudModel[]>("remove_cloud_model", { id });
/** Switch the current chat (and the default for new chats) to a cloud model,
 *  continuing the same conversation. Resolves with the updated session info. */
export const setModel = (model: string) => invoke<SessionInfo>("set_model", { model });

export const onModelProgress = (handler: (p: DownloadProgress) => void) =>
  listen<DownloadProgress>("models://progress", (e) => handler(e.payload));

export const onRuntimeInstall = (handler: (e: RuntimeInstallEvent) => void) =>
  listen<RuntimeInstallEvent>("runtime://install", (e) => handler(e.payload));

/** Phases of switching to a local model (runtime init → loading → ready). */
export const onLocalStatus = (handler: (e: LocalStatus) => void) =>
  listen<LocalStatus>("local://status", (e) => handler(e.payload));

export const onLlamaInstall = (handler: (line: string) => void) =>
  listen<string>("llama://install", (e) => handler(e.payload));

// ---- themes ----------------------------------------------------------------

export const listThemes = () => invoke<ThemeSummary[]>("list_themes");
export const activeTheme = () => invoke<Theme>("active_theme");
export const useTheme = (name: string) => invoke<Theme>("use_theme", { name });
export const importTheme = (contents: string) => invoke<Theme>("import_theme", { contents });
export const exportTheme = (name: string) => invoke<string>("export_theme", { name });
export const removeTheme = (name: string) => invoke<void>("remove_theme", { name });
export const newTheme = (brief: string) => invoke<Theme>("new_theme", { brief });
