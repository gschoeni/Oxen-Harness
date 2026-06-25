// Typed wrappers over Tauri commands and events. Components import from here —
// never call `invoke`/`listen` directly.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ConnectionView,
  DownloadProgress,
  ModelsView,
  QuestionAnswer,
  QuestionPayload,
  SessionInfo,
  SessionSummary,
  CanvasEvent,
  SessionView,
  Theme,
  ThemeSummary,
  TokenEvent,
  ToolEvent,
} from "./types";

// ---- session / agent -------------------------------------------------------

export const sessionInfo = () => invoke<SessionInfo>("session_info");
export const listSessions = () => invoke<SessionSummary[]>("list_sessions");
export const newSession = () => invoke<SessionInfo>("new_session");
export const resumeSession = (id: string) => invoke<SessionView>("resume_session", { id });

/** Run one user turn in `session`; streams via session-tagged `agent://token` /
 *  `agent://tool` events. Resolves with the assistant's final text. Multiple
 *  sessions can run at once, so a chat keeps going after you switch away.
 *  `attachments` are absolute file paths of dropped images/PDFs to send along. */
export const runTurn = (session: string, prompt: string, attachments: string[] = []) =>
  invoke<string>("run_turn", { session, prompt, attachments });

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

// ---- local models ----------------------------------------------------------

export const listModels = () => invoke<ModelsView>("list_models");
export const installLlama = () => invoke<void>("install_llama");
export const pullModel = (id: string) => invoke<void>("pull_model", { id });
export const removeModel = (id: string) => invoke<void>("remove_model", { id });
export const useLocalModel = (id: string) => invoke<SessionInfo>("use_local_model", { id });

export const onModelProgress = (handler: (p: DownloadProgress) => void) =>
  listen<DownloadProgress>("models://progress", (e) => handler(e.payload));

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
