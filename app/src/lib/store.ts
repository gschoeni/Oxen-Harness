import { navigate, travel, type WorkContext } from "../features/workbench/context";
import { availableTarget, resolveView } from "../features/workbench/registry";
import { loadKeyStatus, type KeyStatus } from "./apiKey";
import { CHAT_MIN_FIT, FILES_DEFAULT_WIDTH, RAIL_W, WORK_VIEW_DEFAULT_WIDTH, WORK_VIEW_MIN_WIDTH } from "../features/docks/layout";
import type { ViewTarget } from "../workbench-sdk";
// Global app state. Chats are multi-session: each chat owns a thread, a run
// status, and a send queue keyed by session id, so a chat keeps streaming in the
// background after you switch to (or start) another. The store — not the Chat
// component — drives turns and routes streamed tokens, so progress survives
// unmounting the view. Also holds light/dark mode, the active theme, history,
// open overlays, and any pending clarifying question.

import { create } from "zustand";
import { tailChars } from "./format";
import { applyThemePalette, applyThemeStyle } from "./theme";
import {
  deleteProject,
  sessionMessages,
  deleteSession,
  gitStatus,
  sessionMarkSeen,
  renameSession as renameSessionIpc,
  listCloudModels,
  listProjects,
  listSessions,
  newSession,
  sessionReopen,
  sessionFinish,
  threadsSnapshot,
  resumeSession,
  runCodeReview as runCodeReviewIpc,
  runTurn,
  retryTurn,
  deliverPending,
  cancelAgent,
  cancelFleet,
  cancelTurn,
  interjectAgent,
  killBackgroundTask,
  listAgents,
  listTasks,
  cancelDownload as cancelDownloadIpc,
  downloadModel as downloadModelIpc,
  configureOxenKey,
  sessionInfo,
  selectCloudModelForNewChats,
  setActiveProject,
  setCompressionMode,
  setModel,
  setReviewStatus as setReviewStatusIpc,
  setReviewStatusMany as setReviewStatusManyIpc,
  previewStatus,
  totalCostUsd,
  sessionTreeUsage,
  totalTokensUsed,
  useLocalModel,
  listMedia,
  cancelMedia as cancelMediaIpc,
  getMediaPrefs,
} from "./ipc";
import {
  appendApiKeyPrompt,
  appendNotice,
  appendRetryPrompt,
  appendToken,
  dropRetryPrompts,
  endsMidTurn,
  finalizeAssistant,
  lastUserText,
  resolveRecoveryPrompt,
  resumeMidTurn,
  openReply,
  startTurn,
  toolEnd,
  toolStart,
  toolProgress,
  transcriptToItems,
  type Item,
} from "../features/chat/thread";
import { openThreadIds, threadsFromState } from "../features/threads/threads";
import { partialCanvasDoc } from "./streamingArgs";
import { withSnippetContext } from "./snippets";
import { getUi, setUi } from "./uiState";
import {
  movedTab,
  neighbourTab,
  parseChatTabs,
  stripOf,
  withoutStrip,
  withoutTabs,
  withTab,
  type ChatTabs,
} from "./chatTabs";
import type {
  ApprovalEvent,
  ApprovalRequestEvent,
  CanvasDoc,
  CodeSnippet,
  CanvasEvent,
  CloudModel,
  CodeReviewProgressEvent,
  FleetActivityEvent,
  FleetBudgetEvent,
  FleetAgentEvent,
  FleetStartedEvent,
  FsChangedEvent,
  GitFileState,
  LocalStatus,
  Mode,
  OpenFileEvent,
  ThreadSnapshot,
  Project,
  PendingRoundTrip,
  QuestionPayload,
  ReviewStatus,
  RunStatus,
  SessionInfo,
  SessionSummary,
  SettingsPage,
  StartupModelChoice,
  Theme,
  ToolEvent,
  ToolProgressEvent,
  TurnEndedEvent,
  NoticeEvent,
  ToolDeltaEvent,
  UsageEvent,
  CompactedEvent,
  CompressionEvent,
  CompressionMode,
  DownloadProgress,
  ModelErrorDetail,
  ModelRef,
  PreviewConsoleEvent,
  PreviewEvent,
  PreviewStatus,
  RetryEvent,
  AgentSummary,
  TasksChangedEvent,
  TaskSummary,
  MediaChangedEvent,
  MediaItem,
  MediaUpload,
  MediaPrefs,
} from "./types";

/** One model download the store is tracking — in flight (or failed), keyed by
 *  the [`ModelRef`] id. Lives in the store, not the settings page, so every
 *  download stays visible (and keeps streaming progress) while the user browses
 *  other models, other settings pages, or the chat. */
export interface ActiveDownload {
  model: ModelRef;
  downloaded: number;
  total: number | null;
  /** Best-known fraction complete, 0..1 (0 until the first progress event). */
  fraction: number;
  status: "downloading" | "cancelling" | "error";
  error?: string;
  startedAt: number;
}

interface DockLayout {
  widths: Record<string, number>;
  collapsed: Record<string, boolean>;
}

/** Dock layout (per-side widths + collapsed sides), remembered across runs. */
function loadDockLayout(): DockLayout {
  const raw = getUi("docks");
  return {
    widths: typeof raw?.widths === "object" && raw.widths ? raw.widths : {},
    collapsed: typeof raw?.collapsed === "object" && raw.collapsed ? raw.collapsed : {},
  };
}

function saveDockLayout(layout: DockLayout) {
  setUi("docks", layout);
}

/** The layout patch that expands the right column out of its rail for an
 *  agent's open — the same arithmetic as a click on the rail: take only the
 *  work view's minimum and fold the left side when the window can't fit
 *  both, so the un-collapse is never a dead one the solver folds right back. */
function revealRightColumn(s: Pick<AppState, "dockWidths" | "dockCollapsed">): Pick<AppState, "dockWidths" | "dockCollapsed"> {
  const want = s.dockWidths.right ?? WORK_VIEW_DEFAULT_WIDTH;
  const leftWidth = s.dockCollapsed.left ? RAIL_W : (s.dockWidths.left ?? FILES_DEFAULT_WIDTH);
  const tight = window.innerWidth - leftWidth - want < CHAT_MIN_FIT;
  const foldLeft = tight && window.innerWidth - leftWidth - WORK_VIEW_MIN_WIDTH < CHAT_MIN_FIT;
  const widths = tight ? { ...s.dockWidths, right: WORK_VIEW_MIN_WIDTH } : s.dockWidths;
  const collapsed = { ...s.dockCollapsed, right: false, ...(foldLeft && leftWidth > RAIL_W ? { left: true } : {}) };
  saveDockLayout({ widths, collapsed });
  return { dockWidths: widths, dockCollapsed: collapsed };
}

/** The store patch that installs a tab layout and persists it in one step,
 *  so no caller can change the strip and forget the write. */
function tabsPatch(chatTabs: ChatTabs): Pick<AppState, "chatTabs"> {
  setUi("chatTabs", chatTabs);
  return { chatTabs };
}

/** The patch that gives the chat about to be shown its tab. Every session
 *  swap applies it, so "the visible chat has a tab" holds by construction. */
function tabFor(s: AppState, info: SessionInfo): Partial<AppState> {
  const next = withTab(s.chatTabs, info.workspace, info.session_id);
  return next === s.chatTabs ? {} : tabsPatch(next);
}

/** A chat nothing has touched: no persisted turn, no live thread, not
 *  running. Closing the last such tab would only mint another in its place. */
function isFreshChat(s: AppState, id: string): boolean {
  return (
    !s.sessions.some((x) => x.id === id) &&
    s.runStatus[id] !== "running" &&
    (s.threads[id]?.length ?? 0) === 0
  );
}


/** The right-column dock ids a user can pick as the active tab
 *  (see `features/docks/docks.tsx`). */
export type RightTabId = string;

/** One parallel subagent as shown in the chat's fleet panel. */
export interface FleetLane {
  name: string;
  /** The lane's id once it is running (what a per-lane stop takes). */
  id: string;
  status: "queued" | "running" | "done" | "failed" | "partial" | "cancelled";
  /** One-line rolling readout (tool name or the freshest streamed words). */
  activity: string;
  /** Rolling tail of everything the lane streamed, for the expanded view. */
  tail: string;
  tokens: number;
}

/** A running fleet: its lanes plus which one is expanded for watching. */
export interface FleetView {
  finished?: boolean;
  /** The chat it runs in (fleets are keyed by their own id, see `fleets`). */
  session: string;
  source: "review" | "turn";
  /** The tool call that spawned it (see `FleetStartedEvent.call`). */
  call?: string;
  lanes: FleetLane[];
  focused: number | null;
  /** Where the turn's tree budget stands, once the fleet has reported it. */
  budget?: { tokens: number; max_tokens: number; spawns: number; max_spawns: number };
}

/** A chat's fleets in flight, oldest first (the order they were keyed in). */
export function fleetsFor(
  fleets: Record<string, FleetView | undefined>,
  session: string,
): Array<[string, FleetView]> {
  return Object.entries(fleets).filter(
    (entry): entry is [string, FleetView] => entry[1]?.session === session,
  );
}

function withoutFleetsOf(
  fleets: Record<string, FleetView | undefined>,
  session: string,
): Record<string, FleetView | undefined> {
  return Object.fromEntries(Object.entries(fleets).filter(([, f]) => f?.session !== session));
}

/** Cap on a lane's one-line activity readout — matches the CLI's
 *  `ACTIVITY_TAIL` (fleet_ui.rs) so both hosts show the same rolling window. */
const LANE_ACTIVITY_CAP = 120;
/** Cap on a lane's stored output tail (the expanded watch view) — matches the
 *  CLI's `OUTPUT_TAIL`. */
const LANE_TAIL_CAP = 4000;

/** Mirrors the backend's chars-per-token budgeting heuristic (budget.rs), so the
 *  live streaming estimate lines up with the authoritative count at turn end. */
const CHARS_PER_TOKEN = 4;

/** Backstop on a chat's pending send queue. Realistic queues are a few prompts;
 *  this only bounds pathological growth (e.g. submit held down) so the queue
 *  can't grow without limit in memory. */
const MAX_QUEUE = 50;

/** How many canvas docs to keep in memory per session. The full content of each
 *  also lives in the chat transcript (the canvas tool-call chip rebuilds the doc
 *  via `openCanvasDoc`), so evicting the oldest beyond this cap frees memory
 *  without losing anything — a stale tab is just re-added when its chip is
 *  clicked. Set well above any realistic session so it never trims in practice. */
const MAX_CANVASES = 12;
const MAX_CACHED_THREADS = 4;
/** How many *running* background sessions keep their thread resident beyond
 *  the cache cap. A fleet can have dozens of chats streaming at once,
 *  and every resident thread is up to 300 items with 16 KB tool args each —
 *  so only the most recently active few stay; the rest keep running headless
 *  (their transcript persists on the backend) and rebuild their thread from
 *  it when viewed. The visible session is always protected on top of these. */
const MAX_PROTECTED_RUNNING = 6;
const MAX_STREAMING_TOOL_ARGS = 256_000;

/** When each session last streamed a token or swung a tool (ms epoch). Not
 *  store state: it changes on the hot path and nothing renders it — it only
 *  ranks running sessions for thread-cache protection. */
const lastActive = new Map<string, number>();

/** Keep the current session's thread, the most recently active running
 *  sessions' threads (up to `MAX_PROTECTED_RUNNING`), and then the newest
 *  threads up to `MAX_CACHED_THREADS` in all. Exported for its unit test;
 *  `activity` defaults to the live ranking. */
export function capThreadSessions(
  threads: Record<string, Item[]>,
  runStatus: Record<string, RunStatus>,
  current: string,
  activity: ReadonlyMap<string, number> = lastActive,
): Record<string, Item[]> {
  // Key order is insertion order (oldest first) — the tie-break when two
  // running sessions have no recorded activity yet.
  const order = Object.keys(threads);
  const running = order
    .filter((id) => id !== current && runStatus[id] === "running")
    .sort(
      (a, b) =>
        (activity.get(b) ?? 0) - (activity.get(a) ?? 0) || order.indexOf(b) - order.indexOf(a),
    );
  const protectedIds = new Set([current, ...running.slice(0, MAX_PROTECTED_RUNNING)]);
  const keep = new Set(protectedIds);
  for (const id of order.slice().reverse()) {
    if (keep.size >= MAX_CACHED_THREADS && !protectedIds.has(id)) continue;
    keep.add(id);
  }
  return Object.fromEntries(Object.entries(threads).filter(([id]) => keep.has(id)));
}

/** Drop a per-session record's entries for sessions that are neither cached
 *  nor running. A running session may have lost its thread to the cap above,
 *  but its queue, fleet lanes, review card, info and staged snippets are live
 *  state the turn still depends on — only the thread (rebuildable from the
 *  persisted transcript) is what eviction targets. */
function retainCached<T>(
  record: Record<string, T>,
  threads: Record<string, Item[]>,
  runStatus: Record<string, RunStatus>,
): Record<string, T> {
  const ids = cachedSessionIds(threads, runStatus);
  return Object.fromEntries(Object.entries(record).filter(([id]) => ids.has(id)));
}

/** The sessions whose per-session state survives a sweep: cached or running. */
function cachedSessionIds(
  threads: Record<string, Item[]>,
  runStatus: Record<string, RunStatus>,
): Set<string> {
  const ids = new Set(Object.keys(threads));
  for (const [id, status] of Object.entries(runStatus)) if (status === "running") ids.add(id);
  return ids;
}

/** `retainCached` for fleets, which are keyed by fleet id rather than session:
 *  a fleet lives as long as the chat it runs in does. */
export function retainFleets(
  fleets: Record<string, FleetView | undefined>,
  sessions: Set<string>,
): Record<string, FleetView | undefined> {
  return Object.fromEntries(
    Object.entries(fleets).filter(([, fleet]) => fleet !== undefined && sessions.has(fleet.session)),
  );
}

/** The one sweep every session switch runs: install the capped `threads` and
 *  trim every per-session slice that can hold real content (canvas docs,
 *  streaming previews, staged code snippets, preview errors, …) to match.
 *  Adding a per-session record to the store? If it can grow, add it here. */
function sweepCached(s: AppState, threads: Record<string, Item[]>): Partial<AppState> {
  const keep = <T,>(record: Record<string, T>) => retainCached(record, threads, s.runStatus);
  return {
    threads,
    infos: keep(s.infos),
    canvases: keep(s.canvases),
    activeCanvas: keep(s.activeCanvas),
    codeReview: keep(s.codeReview),
    fleets: retainFleets(s.fleets, cachedSessionIds(threads, s.runStatus)),
    queues: keep(s.queues),
    canvasWriting: keep(s.canvasWriting),
    streamingTool: keep(s.streamingTool),
    streamingCanvas: keep(s.streamingCanvas),
    liveTokens: keep(s.liveTokens),
    sessionUsage: keep(s.sessionUsage),
    treeUsage: keep(s.treeUsage),
    tokensPerSecond: keep(s.tokensPerSecond),
    compression: keep(s.compression),
    snippets: keep(s.snippets),
    previewErrors: keep(s.previewErrors),
  };
}

// ---- token coalescing --------------------------------------------------------
//
// The backend already batches deltas (~512 B / 150 ms per StreamBatch), yet a
// fast model still lands many events a second, and each one used to copy the
// session's whole thread array plus the `threads`/`liveTokens` records. Tokens
// now accumulate here per session and land in the store on a short timer, so a
// burst costs one copy. The window is short because the backend has already
// smoothed the stream; anything longer only adds latency.
/** A chat's running spend, subagents included (see `refreshTreeUsage`). */
export interface TreeUsage {
  tokens: number;
  cost: number | null;
  unpriced: boolean;
}
/** Lane activity arrives many times a second; the bill is re-read once per
 *  burst. */
const TREE_USAGE_DEBOUNCE_MS = 1500;
const treeUsageTimers = new Map<string, number>();

const TOKEN_FLUSH_MS = 50;
const pendingTokens = new Map<string, { text: string; est: number; tps: number | null }>();
let tokenFlushTimer: number | null = null;

/** Keep only the newest `MAX_CANVASES` docs. The just-touched doc is appended
 *  last (and is the active one), so it always survives the trim. */
function capCanvases(list: CanvasDoc[]): CanvasDoc[] {
  return list.length > MAX_CANVASES ? list.slice(list.length - MAX_CANVASES) : list;
}

/** Merge a path group into a session's editor tabs: front the tab if that
 *  exact group is already open, otherwise append it and make it active. */
function addEditorTab(
  pane: { tabs: string[][]; active: number } | undefined,
  paths: string[],
): { tabs: string[][]; active: number } {
  const tabs = pane?.tabs ?? [];
  const key = paths.join("\n");
  const i = tabs.findIndex((t) => t.join("\n") === key);
  if (i >= 0) return { tabs, active: i };
  return { tabs: [...tabs, paths], active: tabs.length };
}

export interface QueuedPrompt {
  text: string;
  attachments: string[];
}

/** Whether a turn's error is an Oxen authentication failure (no/invalid API key),
 *  so the chat can offer an inline key-entry form instead of a dead-end error.
 *  Matches the backend's `Oxen API error (401): …` shape and the auth wording. */
function isAuthError(message: string): boolean {
  return /\(401\)/.test(message) || /\b(must be authenticated|unauthorized)\b/i.test(message);
}

/** Whether a turn's error is an out-of-credits failure (a 402), so the chat can
 *  offer an inline "add credits, then retry" card instead of a dead-end error.
 *  Matches the backend's `Oxen API error (402): …` shape and Oxen's
 *  insufficient-credits wording. */
export function isCreditsError(message: string): boolean {
  return /\(402\)/.test(message) || /\b(out of credits|insufficient[_ ]credits)\b/i.test(message);
}

function reconcileQueueTexts(previous: QueuedPrompt[] = [], texts: string[]): QueuedPrompt[] {
  const remaining = [...previous];
  return texts.map((text) => {
    const existing = remaining.findIndex((q) => q.text === text);
    if (existing >= 0) {
      const [prompt] = remaining.splice(existing, 1);
      return prompt;
    }
    return { text, attachments: [] };
  });
}

interface AppState {
  workContexts: Record<string, WorkContext>;
  openWorkView: (session: string, target: ViewTarget, agent?: boolean) => void;
  travelWorkView: (offset: number) => void;
  pinWorkView: () => void;
  mode: Mode;
  theme: Theme | null;
  /** Which empty-state hero game the player has chosen (persisted). Null falls
   *  back to the active theme's default game. Shared by the hero and the
   *  play-while-you-work game dock so both show the same cabinet. */
  heroGame: string | null;
  /** Whether the floating game dock is open (lets you play during a live turn). */
  gameDockOpen: boolean;
  /** Whether an Oxen API key resolves, and which models need none. Null until
   *  first read (or if the read failed) — unknown never prompts for a key. */
  keyStatus: KeyStatus | null;
  /** Re-read `keyStatus` from the host. */
  refreshKeyStatus: () => Promise<void>;
  /** Save an Oxen API key from the up-front prompt and authenticate the
   *  chat's running agent. Rejects if saving fails, so the form can say why. */
  saveApiKey: (session: string, key: string) => Promise<void>;
  /** The chat currently shown. */
  session: SessionInfo | null;
  /** All-time total tokens used across every session (drives the hero's stat). */
  totalTokensUsed: number;
  /** Estimated all-time Oxen cloud spend across recorded models, shown under
   *  the token total. `null` when catalog pricing cannot be resolved. */
  totalCostUsd: number | null;
  sessions: SessionSummary[];
  /** Known projects (working directories), refreshed alongside history. */
  projects: Project[];
  /** The cloud model catalog (built-ins + custom), for the picker + settings. */
  cloudModels: CloudModel[];
  /** Whether Home — the project cards, and the way into any chat — is
   *  open. It is the application's navigation root. */
  homeOpen: boolean;
  /** Project whose getting-started/files page was opened explicitly. Null
   *  shows the project cards instead. */
  projectHomePath: string | null;
  /** The last backend snapshot of every thread's facts; null until the first
   *  read. Holds the authoritative running set; live events layer on top via
   *  `runStatus`. */
  threadsSnapshot: ThreadSnapshot | null;
  /** Known session infos by id, so switching to a live chat keeps its header. */
  infos: Record<string, SessionInfo>;
  /** Live thread items per session id. */
  threads: Record<string, Item[]>;
  /** Estimated tokens streamed in the current in-flight turn, per session — lets
   *  the usage meter tick up live before the authoritative count lands at turn
   *  end. Reset to 0 when that turn's `agent://usage` arrives. */
  liveTokens: Record<string, number>;
  /** Cumulative input/output tokens for pricing the active session. */
  sessionUsage: Record<string, { prompt: number; completion: number }>;
  /** What a chat has spent so far with its subagents included, priced per
   *  model from the store — refreshed (debounced) as lanes spend, so the
   *  meter tracks the real running bill while a fleet works. */
  treeUsage: Record<string, TreeUsage>;
  /** Re-read a chat's tree spend; coalesces bursts of lane activity. */
  refreshTreeUsage: (session: string) => void;
  /** Generation speed (tokens/sec) per session, measured over the current
   *  streaming burst. Persists the last rate when idle. */
  tokensPerSecond: Record<string, number>;
  /** Per-session context-compression readout: the mode that ran and the
   *  session's cumulative tokens saved (or, in audit mode, would-be saved),
   *  updated per model call from `agent://compression`. Absent = no savings. */
  compression: Record<string, { mode: CompressionMode; tokensSaved: number }>;
  /** Per-session run state driving the sidebar indicator (absent = idle/read). */
  runStatus: Record<string, RunStatus>;
  /** A running code review's live progress per session (absent = none running):
   *  the current pipeline step plus a rolling snippet of the step agent's
   *  activity, driving the chat's progress card. */
  codeReview: Record<
    string,
    { step: string; index: number; total: number; activity: string } | undefined
  >;
  /** A running fleet's lanes per session (absent = none running): one entry per
   *  parallel subagent, driving the chat's fleet panel. Click a lane to watch
   *  its live output tail. Fed by review fan-out steps and `spawn_agents`
   *  alike. */
  /** Every fleet in flight, keyed by fleet id — not by session, because a
   *  background (`wait: false`) fleet can overlap a later one in one chat and
   *  the two must not share lanes. `fleetsFor` picks a chat's, in start order. */
  fleets: Record<string, FleetView | undefined>;
  /** The agents hub: a chat's subagent lanes, running and finished, as last
   *  fetched (refreshed when a fleet ends and when the panel mounts). */
  agents: Record<string, AgentSummary[] | undefined>;
  /** A chat's background shell tasks, kept current by `tasks://changed`. */
  tasks: Record<string, TaskSummary[] | undefined>;
  /** Each project's media library (generations, in flight and done), keyed
   *  by workspace root and kept current by `media://changed`. */
  media: Record<string, MediaItem[] | undefined>;
  /** Reference uploads in flight per workspace root (live-only). */
  mediaUploads: Record<string, MediaUpload[]>;
  /** The media preferences (default models, folder, budgets), fetched once
   *  on first use so cards can name the model a call defaulted to. */
  mediaPrefs: MediaPrefs | null;
  ensureMediaPrefs: () => void;
  /** The generation the Gallery should select and scroll to (a chat card was
   *  clicked); cleared once the panel has honored it. */
  mediaFocus: string | null;
  /** Files another surface (the Gallery's "Use as reference") staged for the
   *  composer; Chat drains them into its attachment list. Absolute paths. */
  pendingAttachments: string[];
  /** The inspector follows a running lane live (polls its transcript). */
  inspectorLive: boolean;
  /** Prompts queued while a session is mid-turn, sent in order as it frees up. */
  queues: Record<string, QueuedPrompt[]>;
  /** Documents the agent showed in the canvas, per session (ordered, by id). */
  canvases: Record<string, CanvasDoc[]>;
  /** The canvas doc id currently open in the side panel per session (null/absent
   *  = panel closed). */
  activeCanvas: Record<string, string | null>;
  /** True while the model is writing/updating a canvas for a session (before its
   *  content arrives), so the panel can show a "writing…" state. */
  canvasWriting: Record<string, boolean>;
  /** The tool call whose arguments are currently streaming in, per session —
   *  drives the live file-write preview. `args` is the accumulated raw JSON. */
  streamingTool: Record<string, { name: string; args: string } | undefined>;
  /** A provisional canvas doc built from the in-flight canvas call's streaming
   *  args, so the panel shows the document forming before it's committed. */
  streamingCanvas: Record<string, CanvasDoc | undefined>;
  /** Each session's dev-server status (absent = never started). Drives the
   *  live-preview pane and the sidebar port chips. */
  previews: Record<string, PreviewStatus | undefined>;
  /** Sessions whose preview pane the user closed (a later "ready" reopens it). */
  previewClosed: Record<string, boolean>;
  /** The preview page's most recent JavaScript error per session (absent =
   *  none) — drives the pane's "Fix it" banner. */
  previewErrors: Record<string, string | undefined>;
  /** View explicitly opened for each session in this app run. Saved work
   *  contexts retain history, but do not open a panel on startup. */
  rightTab: Record<string, RightTabId>;
  /** The URL open in the link-browser side panel (null = pane closed).
   *  App-wide, not per-session: it's a page the user is reading, not part of
   *  any chat's state. */
  browserUrl: string | null;
  /** Which left-dock is active when more than one has content (a dock id from
   *  the registry). App-wide: the file tree follows the workspace, not the chat. */
  leftTab: string | null;
  /** The chat tabs open per project (workspace path → session ids in strip
   *  order). The visible chat is the active tab and always has one. A tab is
   *  a bookmark, not a process: closing it never stops the agent behind it.
   *  Persisted across runs. */
  chatTabs: ChatTabs;
  /** Whether the chat history modal (search every chat, open one as a tab)
   *  is showing. */
  historyOpen: boolean;
  /** A workspace-relative path the Files dock should expand to, select, and
   *  scroll into view (the Gallery's "Reveal in Files"). `tick` makes the
   *  same path revealable twice; the panel clears it once done. */
  filesReveal: { path: string; tick: number } | null;
  /** The Editor/viewer dock's open tabs per session. Each tab is a group of
   *  workspace-relative paths: one text file for the code editor, one media
   *  file for the media view, or several images for the gallery grid.
   *  `active` indexes into `tabs`. Absent = pane closed. */
  editorTabs: Record<string, { tabs: string[][]; active: number } | undefined>;
  /** The latest on-disk change batch from the workspace watcher, stamped with
   *  a monotonic tick so equal-looking batches still notify subscribers (the
   *  Files tree and open editor views react to this). */
  fsChange: (FsChangedEvent & { tick: number }) | null;
  /** Code selections staged as context for the next prompt, per session —
   *  shown as chips by the composer, baked into the prompt at send time. */
  snippets: Record<string, CodeSnippet[]>;
  /** Changed files per workspace root (git status). `null` = not a git
   *  repository; absent = not loaded yet. Read by the Files tree's badges and
   *  the editor's "view diff" affordance. */
  gitStates: Record<string, GitFileState[] | null>;
  /** Whether the code editor and diff viewer wrap long lines. Persisted. */
  editorWrap: boolean;
  /** Each dock column's width in px, keyed by side. Drag-resized, persisted. */
  dockWidths: Record<string, number>;
  /** Sides the user collapsed to a rail, keyed by side. Persisted. */
  dockCollapsed: Record<string, boolean>;
  settingsOpen: boolean;
  /** Which subpage the full-screen Settings surface shows when open. */
  settingsPage: SettingsPage;
  /** The transcript inspector drawer. `review` is set when opened from the
   *  dataset builder (carries the queue of chats to page through); null =
   *  plain inspection (e.g. the chat's </> button). Absent = drawer closed. */
  inspector: { sessionId: string; review: { queue: string[]; index: number } | null } | null;
  question: QuestionPayload | null;
  /** Pending permission approvals, keyed by session — a background chat's
   *  approval card must not pop into the visible one. */
  approvals: Record<string, ApprovalRequestEvent | undefined>;

  setMode: (m: Mode) => void;
  toggleMode: () => void;
  /** Choose (and persist) the empty-state hero game. */
  setHeroGame: (name: string) => void;
  /** Open/close the floating game dock. */
  setGameDockOpen: (open: boolean) => void;
  applyTheme: (t: Theme) => void;
  /** Re-read the history list (and projects). Resolves true when it loaded,
   *  false when the previous lists were kept after a transient failure. */
  refreshHistory: () => Promise<boolean>;
  loadSession: () => Promise<void>;
  startNewSession: () => Promise<void>;
  /** The "+" and ⌘T: a fresh chat — unless the visible one is still untouched,
   *  in which case it's already the fresh chat and minting another would only
   *  pile up empty sessions. */
  newChat: () => Promise<void>;
  resume: (id: string) => Promise<void>;
  /** Name a chat (a tab's double-click). Blank clears the name, so the chat
   *  titles itself by its first message again. The list and the board update
   *  in place first; the backend write follows. */
  renameSession: (id: string, title: string) => Promise<void>;
  /** Permanently delete a chat; if it was the current one, open a fresh chat. */
  removeSession: (id: string) => Promise<void>;
  /** Permanently delete many chats at once: one state sweep, one
   *  history+threads refresh at the end. */
  removeSessions: (ids: string[]) => Promise<void>;
  /** Open/close Home. Opening refreshes the thread verdicts. */
  setHomeOpen: (open: boolean) => void;
  /** Re-read every thread's facts from the backend. */
  refreshThreads: () => Promise<void>;
  /** Mark a thread finished — the one human-set boolean. */
  finishThread: (id: string) => Promise<void>;
  /** Reopen a finished thread. */
  reopenThread: (id: string) => Promise<void>;
  /** Curate a thread for the training dataset ("" | "kept" | "rejected"),
   *  keeping the thread verdicts and the history list in step. */
  setThreadReview: (id: string, status: ReviewStatus) => Promise<void>;
  /** Open a project's getting-started, guidance, and reference-files page. */
  openProjectHome: (path: string) => void;
  /** Make project-scoped surfaces point at a project without creating a chat. */
  selectProject: (path: string) => Promise<void>;
  /** Open every thread of `workspace` that still has something owed — needs
   *  the user, or simply isn't marked finished yet — as tabs, most pressing first.
   *  Entering a project calls this so its loose ends are in the strip on
   *  arrival; tabs already open keep their place. */
  openProjectThreads: (workspace: string) => Promise<void>;
  /** Prepare a known project and fresh chat while keeping its home visible. */
  prepareProject: (path: string, model?: StartupModelChoice) => Promise<void>;
  /** Switch to a known project and enter its fresh chat. */
  enterProject: (path: string) => Promise<void>;
  /** Forget a project without deleting its files or chat history. */
  removeProject: (path: string) => Promise<void>;
  /** Adopt a fresh session created by a model/connection switch as the current
   *  chat (it starts empty on a new endpoint). */
  adoptSession: (info: SessionInfo) => void;
  /** Refresh the cloud model catalog from the backend. */
  loadCloudModels: () => Promise<void>;
  /** Swap the current chat to a cloud model in place, continuing the same
   *  conversation (keeps the thread; only the model changes). */
  changeModel: (model: string) => Promise<void>;
  /** Switch context compression for the live chat (persisted for new ones too). */
  changeCompressionMode: (mode: CompressionMode) => Promise<void>;
  /** Switch to a downloaded local model — starts a fresh chat on it. */
  switchToLocalModel: (id: string) => Promise<void>;
  /** Live status while switching to a local model (its server is starting), so
   *  the UI can show progress instead of an opaque "Switching…". Null when idle. */
  localSwitch: { model: string; phase: LocalStatus["phase"]; startedAt: number } | null;
  /** Update the local-switch phase from a `local://status` event. */
  setLocalStatus: (s: LocalStatus) => void;
  /** Every model download in flight (or failed), keyed by model id. */
  downloads: Record<string, ActiveDownload>;
  /** Bumped each time a download lands, so open views re-read the catalog. */
  downloadsRev: number;
  /** Start downloading a model. A no-op if that id is already in flight. */
  startDownload: (model: ModelRef) => void;
  /** Stop an in-flight download (backend aborts the stream + reclaims bytes). */
  stopDownload: (id: string) => void;
  /** Clear a failed download row. */
  dismissDownload: (id: string) => void;
  /** Update a download's progress from a `models://progress` event. */
  ingestDownloadProgress: (p: DownloadProgress) => void;
  /** Send (or queue) a prompt in the current chat. */
  send: (text: string, attachments?: string[]) => void;
  /** Run the code-review pipeline in the current chat's workspace (uncommitted
   *  changes, or PR-style against `baseBranch`). The findings land in the thread
   *  as a settled exchange, so a follow-up "fix 1 and 3" just works. */
  startCodeReview: (baseBranch?: string) => void;
  /** Add local command output to the current thread. */
  addNotice: (text: string) => void;
  /** Advance a session's review progress card to the next pipeline step. */
  ingestCodeReviewProgress: (e: CodeReviewProgressEvent) => void;
  /** Update the review card's live activity line (streamed text or a tool name). */
  ingestCodeReviewActivity: (session: string, text: string, replace: boolean) => void;
  /** Open a session's fleet panel with its lanes (all queued). */
  ingestFleetStarted: (e: FleetStartedEvent) => void;
  /** A lane changed state (started / done / failed). */
  ingestFleetAgent: (e: FleetAgentEvent) => void;
  /** Live activity from one lane (text, a tool, or a token-count update). */
  ingestFleetActivity: (e: FleetActivityEvent) => void;
  /** The turn's tree budget moved. */
  ingestFleetBudget: (e: FleetBudgetEvent) => void;
  /** The fleet finished: close its panel. */
  ingestFleetCompleted: (session: string, fleet: string) => void;
  /** Expand one lane of `fleet` to watch its output (null collapses back). */
  setFleetFocus: (fleet: string, index: number | null) => void;
  /** Stop one fleet without ending the turn; its panel closes on the
   *  backend's `fleet://completed` once the lanes settle. */
  stopFleet: (session: string, fleet: string) => Promise<boolean>;
  /** Stop one lane of a fleet; the rest of the fleet carries on. */
  stopLane: (session: string, lane: string) => Promise<boolean>;
  /** Re-fetch a chat's agents hub. */
  refreshAgents: (session: string) => Promise<void>;
  /** Hand a running lane a message for its next round. */
  steerLane: (session: string, lane: string, text: string) => Promise<boolean>;
  /** Open the inspector on a running lane and follow it live. */
  watchLane: (lane: string) => void;
  /** The subagent open in a chat's thread column, per session: its lane id,
   *  or absent/null for the chat itself. Opening a lane swaps the messages
   *  and composer for that agent's transcript and a composer that steers it
   *  (running) or follows it up (finished); the parent keeps streaming
   *  underneath. Per session, so switching tabs keeps each chat's view. */
  agentView: Record<string, string | null | undefined>;
  openAgent: (session: string, lane: string) => void;
  closeAgent: (session: string) => void;
  /** The chat's background tasks changed on the backend. */
  ingestTasksChanged: (e: TasksChangedEvent) => void;
  /** `turn://delivery-ready`: background work finished. An idle chat runs a
   *  turn that delivers it; a busy one delivers it once its turns settle. */
  ingestDeliveryReady: (session: string) => void;
  /** Re-fetch a chat's background tasks. */
  refreshTasks: (session: string) => Promise<void>;
  /** Kill one background task. */
  killTask: (session: string, id: number) => void;
  /** A project's media library changed on the backend. */
  ingestMediaChanged: (e: MediaChangedEvent) => void;
  /** Re-fetch a project's media library. */
  refreshMedia: (root: string) => Promise<void>;
  /** Cancel an in-flight generation. */
  cancelMedia: (id: string) => void;
  /** Show the Gallery dock for the current chat, selecting `itemId` if given. */
  openGallery: (itemId?: string) => void;
  /** Consume the Gallery's focus request. */
  clearMediaFocus: () => void;
  /** Stage a file for the composer's next message (absolute path). */
  stageAttachment: (path: string) => void;
  /** The composer took the staged files. */
  takePendingAttachments: () => string[];
  /** Stop the current chat's in-flight turn, killing the model stream. */
  stop: () => void;
  /** Save the Oxen API key entered in a chat's inline auth prompt, then retry the
   *  turn that hit the 401 — keeping the same conversation. Rejects if saving the
   *  key fails, so the form can surface the error. */
  submitApiKey: (session: string, itemId: string, key: string) => Promise<void>;
  /** Continue a chat from its inline retry card (a 402/out-of-credits failure, or
   *  a resumed transcript that ended mid-turn): retire the card and re-drive the
   *  transcript's trailing turn — no duplicate user message. */
  retryBrokenTurn: (session: string, itemId: string) => void;
  /** Replace the current chat's send queue (used by the queue editor). */
  setQueue: (items: string[]) => void;
  /** Route a streamed token / tool event into its session's thread. */
  ingestToken: (session: string, token: string) => void;
  /** Land any buffered streamed tokens in the store now. Every store mutation
   *  does this itself first, so callers only need it to observe the thread
   *  synchronously (tests; teardown). */
  flushTokens: () => void;
  ingestTool: (e: ToolEvent) => void;
  /** Accumulate a streaming tool-args fragment (live file/canvas preview). */
  ingestToolDelta: (e: ToolDeltaEvent) => void;
  /** Update a session's live usage (per-session count + context fill) as it
   *  accrues within a turn. */
  ingestUsage: (e: UsageEvent) => void;
  /** Add a notice to a session's thread when its context was compacted. */
  ingestCompacted: (e: CompactedEvent) => void;
  /** A one-line notice about something the agent did on its own (a
   *  background task's output delivered to the model). */
  ingestNotice: (e: NoticeEvent) => void;
  /** A running tool's live output chunk, appended to its chip. */
  ingestToolProgress: (e: ToolProgressEvent) => void;
  /** Add a notice when a model call hit a transient error and is retrying. */
  ingestRetry: (e: RetryEvent) => void;
  /** Update a session's compression savings counters. Fires per model call —
   *  deliberately no thread notice (that would be far too chatty). */
  ingestCompression: (e: CompressionEvent) => void;
  /** Refresh the all-time total tokens used from the backend. */
  refreshTotalTokens: () => Promise<void>;
  /** Upsert a canvas document and open it in the side panel. */
  ingestCanvas: (e: CanvasEvent) => void;
  /** The model's `open_file` tool: show project files in the session's
   *  Editor/viewer dock (session-tagged, so a background chat's file doesn't
   *  pop into the foreground). */
  ingestOpenFile: (e: OpenFileEvent) => void;
  /** An `fs://changed` batch arrived — files changed on disk (any process). */
  ingestFsChange: (e: FsChangedEvent) => void;
  /** Show a specific canvas doc (or close the panel with null) for the current chat. */
  setActiveCanvas: (id: string | null) => void;
  /** Open (or reopen) a canvas document in the current chat — used to revisit a
   *  past canvas from its chat tool-call chip, including in a resumed chat. */
  openCanvasDoc: (doc: CanvasDoc) => void;
  /** Mark a session as (not) currently writing a canvas. */
  setCanvasWriting: (session: string, writing: boolean) => void;
  /** Route a dev-server lifecycle change into the preview pane + sidebar chips. */
  ingestPreviewStatus: (e: PreviewEvent) => void;
  /** Show the "Fix it" banner for a preview page error. */
  ingestPreviewConsole: (e: PreviewConsoleEvent) => void;
  /** Dismiss `session`'s preview error banner; with `fix`, also send a prompt
   *  asking the agent to fix the error (only if that chat is still open). */
  resolvePreviewError: (session: string, fix: boolean) => void;
  /** Sync a session's dev-server status from the backend (cold mounts/resumes). */
  syncPreview: (session: string) => Promise<void>;
  /** Close the current chat's preview pane (the server keeps running). */
  closePreview: () => void;
  /** Open a URL in the link-browser side panel (a chat link was clicked). */
  openBrowser: (url: string) => void;
  /** Close the link-browser side panel. */
  closeBrowser: () => void;
  /** Switch the current chat's right-panel tab (preview / canvas / browser / editor). */
  setRightTab: (tab: RightTabId) => void;
  /** Switch the left column's active dock (a dock id from the registry). */
  setLeftTab: (id: string) => void;
  /** Close a chat tab. Closing the visible chat lands on its neighbour (the
   *  tab to its right, else left); closing the last one opens a fresh chat.
   *  The agent behind a closed tab keeps running — the history badge counts
   *  it once it needs the user. */
  closeTab: (id: string) => Promise<void>;
  /** Close every tab in `id`'s project except `id`, and show it. */
  closeOtherTabs: (id: string) => Promise<void>;
  /** Close the tabs after `id` in strip order; show `id` if the visible chat
   *  was among them. */
  closeTabsRight: (id: string) => Promise<void>;
  /** Reorder a tab within its strip: just before `before`, or last when null. */
  moveTab: (id: string, before: string | null) => void;
  /** Drop tabs for chats the history no longer lists — run once the list has
   *  loaded at boot, so a chat deleted or never started in a previous run
   *  doesn't come back as a dead tab. The visible chat is always kept. */
  pruneTabs: () => void;
  setHistoryOpen: (open: boolean) => void;
  /** Show the Files dock and reveal a workspace-relative path in its tree. */
  revealInFiles: (path: string) => void;
  clearFilesReveal: () => void;
  /** Open workspace files in the Editor/viewer dock as a tab: one text file
   *  for the editor, one media file for the media view, or several images as
   *  a grid. An already-open tab is fronted instead of duplicated. */
  openInViewer: (paths: string[]) => void;
  /** Bring an already-open editor tab to the front by index. */
  activateEditorTab: (index: number) => void;
  /** Close one editor tab; closing the last one closes the pane. */
  closeEditorTab: (index: number) => void;
  /** Close the current chat's Editor/viewer pane (all tabs). */
  closeViewer: () => void;
  /** Re-read a workspace's git status into `gitStates` (silent on failure —
   *  the state is decoration on top of the tree, never an error surface). */
  refreshGitStatus: (root: string) => Promise<void>;
  /** Flip line wrapping for the code editor + diff viewer (persisted). */
  toggleEditorWrap: () => void;
  /** Stage a code selection as context for the current chat's next prompt. */
  addSnippet: (snippet: CodeSnippet) => void;
  /** Unstage one snippet chip by index. */
  removeSnippet: (index: number) => void;
  /** Resize a dock column (persisted). */
  setDockWidth: (side: string, width: number) => void;
  /** Collapse/expand a dock column to a rail (persisted). */
  setDockCollapsed: (side: string, collapsed: boolean) => void;
  /** Toggle a dock column's collapsed state (the ⌘B / ⌘⌥B shortcuts). */
  toggleDock: (side: string) => void;
  setSettingsOpen: (open: boolean) => void;
  /** Open the Settings surface, optionally jumping straight to a subpage. */
  openSettings: (page?: SettingsPage) => void;
  /** Switch the active Settings subpage (the surface stays open). */
  setSettingsPage: (page: SettingsPage) => void;
  /** Open the inspector drawer to read one chat's raw transcript. */
  openInspector: (sessionId: string) => void;
  /** Open the inspector in review mode over a queue of chats (dataset builder). */
  openReview: (queue: string[], index: number) => void;
  /** Move within the review queue by `delta` (clamped); no-op outside review. */
  reviewStep: (delta: number) => void;
  closeInspector: () => void;
  /** Persist a chat's keep/reject status and reflect it in the session list. */
  setReviewStatus: (id: string, status: ReviewStatus) => Promise<void>;
  /** Bulk-apply a keep/reject status to many chats (dataset builder bulk actions). */
  setReviewStatusMany: (ids: string[], status: ReviewStatus) => Promise<void>;
  setQuestion: (q: QuestionPayload | null) => void;
  /** A turn ended on the event stream. Settles a turn this client *rejoined*
   *  (reloaded mid-turn, so it holds no `runTurn` promise for it); a turn it
   *  drove itself is settled by that promise and ignored here. */
  ingestTurnEnded: (e: TurnEndedEvent) => void;
  /** A gated tool call is waiting on (or done with) the user's decision. */
  ingestApprovalRequest: (e: ApprovalRequestEvent) => void;
  /** Clear the card and leave a notice line once the approval resolves. */
  ingestApprovalResolved: (e: ApprovalEvent) => void;
  /** Drop a session's pending approval card (after answering). */
  clearApproval: (session: string) => void;
}

export const useStore = create<AppState>((rawSet, get) => {
  // Every mutation drains the token buffer first, so no action — a tool start,
  // a turn's end, a session switch, a thread cap — can ever observe a thread
  // that is behind the token stream. Ordering holds by construction rather
  // than by remembering to flush in each handler; a no-op when nothing is
  // pending (one Map size check).
  const set: typeof rawSet = (partial) => {
    flushTokens();
    rawSet((previous) => {
      const patch = typeof partial === "function" ? partial(previous) : partial;
      if (patch.workContexts || (!patch.rightTab && !patch.editorTabs && !("browserUrl" in patch) && !patch.activeCanvas)) return patch;
      const next = { ...previous, ...patch };
      const workContexts = { ...previous.workContexts };
      const rightTab = { ...next.rightTab };
      const onScreen = next.session?.session_id;
      // What the chat on screen was shown, and what its pin kept out.
      let revealed = false;
      const keptOut: string[] = [];
      for (const [session, view] of Object.entries(next.rightTab)) {
        const pane = next.editorTabs[session];
        const changed = view !== previous.rightTab[session] || pane !== previous.editorTabs[session] || next.activeCanvas[session] !== previous.activeCanvas[session] || (view === "browser" && next.session?.session_id === session && next.browserUrl !== previous.browserUrl);
        if (!changed) continue;
        const paths = pane?.tabs[pane.active];
        const target: ViewTarget = { view };
        if (view === "editor" && paths?.length) {
          target.paths = paths; target.path = paths.length === 1 ? paths[0] : undefined;
          target.view = target.path ? resolveView(target.path) : "editor";
        }
        if (view === "canvas") target.id = next.activeCanvas[session] ?? undefined;
        if (view === "browser") target.url = next.browserUrl ?? undefined;
        if (workContexts[session]?.pinned) {
          rightTab[session] = workContexts[session].current.view;
          if (session === onScreen) keptOut.push(target.path ?? target.paths?.join(", ") ?? target.view);
          continue;
        }
        rightTab[session] = target.view;
        workContexts[session] = navigate(workContexts[session], target);
        if (session === onScreen) revealed = true;
      }
      setUi("workContexts", workContexts);
      // An open the agent made for the chat on screen must be seen: a right
      // column folded to its rail is expanded the way a click on the rail
      // would, making room honestly when the window is tight. A background
      // chat's open stays in its own context.
      const layout = revealed ? revealRightColumn(next) : {};
      // A pin keeps the agent's opens out of the work view on purpose; the
      // chat says so, so "they are looking at it now" isn't silently false.
      const threads = keptOut.length && onScreen
        ? { threads: { ...next.threads, [onScreen]: appendNotice(next.threads[onScreen] ?? [], `Opened ${keptOut.join(", ")} behind your pinned work view — unpin it to follow along`) } }
        : {};
      return { ...patch, rightTab, workContexts, ...layout, ...threads };
    });
  };

  function flushTokens() {
    if (tokenFlushTimer != null) {
      window.clearTimeout(tokenFlushTimer);
      tokenFlushTimer = null;
    }
    if (pendingTokens.size === 0) return;
    const batch = [...pendingTokens.entries()];
    pendingTokens.clear();
    rawSet((s) => {
      const threads = { ...s.threads };
      const liveTokens = { ...s.liveTokens };
      const tokensPerSecond = { ...s.tokensPerSecond };
      let touched = false;
      for (const [id, p] of batch) {
        // A session with no cached thread (evicted, or never viewed) drops its
        // tokens — the transcript persists on the backend and viewing the chat
        // rebuilds from there. Same rule the per-event path always applied.
        if (threads[id] === undefined) continue;
        touched = true;
        threads[id] = appendToken(threads[id], p.text);
        // Tick the usage meter up live as the reply streams, matching the
        // backend's ~4-chars-per-token estimate; snapped exact at turn end.
        liveTokens[id] = (liveTokens[id] ?? 0) + p.est;
        if (p.tps !== null) tokensPerSecond[id] = p.tps;
      }
      return touched ? { threads, liveTokens, tokensPerSecond } : {};
    });
  }

  /** Apply `fn` to a session's cached thread. A session with no thread in the
   *  cache (evicted under the running cap) is left alone rather than given a
   *  stub thread holding only this change: the transcript is persisted, and
   *  viewing the chat rebuilds the whole thread from it. */
  const withThread = (s: AppState, id: string, fn: (items: Item[]) => Item[]): Partial<AppState> =>
    s.threads[id] === undefined ? {} : { threads: { ...s.threads, [id]: fn(s.threads[id]) } };

  // Non-reactive per-session sample for the tokens/sec readout: the start of the
  // current streaming burst and tokens seen in it. A burst resets after a gap
  // (tool calls), so the rate reflects active decoding, not idle time.
  const genSamples = new Map<string, { start: number; tokens: number; last: number }>();
  const BURST_GAP_MS = 1200;

  // A turn (or review) just ended with nothing queued: read if the chat is in
  // view, unread if it finished offscreen. Viewing counts as seeing — the
  // thread's durable seen mark moves too, so nothing ever flags a reply the
  // user watched land as "finished while you were away".
  function settleRunStatus(id: string) {
    set((s) => {
      const runStatus = { ...s.runStatus };
      if (s.session?.session_id === id) delete runStatus[id];
      else runStatus[id] = "unread";
      return { runStatus };
    });
  }

  // Mark a thread seen, then refresh the verdicts — in that order, so the
  // snapshot it reads already carries the mark (a refresh racing the write
  // would flag the thread for a beat).
  async function markSeenThenRefresh(id: string) {
    await sessionMarkSeen(id).catch(() => {});
    await get().refreshThreads();
  }

  // A turn just ended: seen if the chat is in view, else just repaint.
  // Offscreen threads have nothing to mark — their activity IS the "while
  // you were away" story.
  function refreshThreadsAfterSeen(id: string) {
    return get().session?.session_id === id ? markSeenThenRefresh(id) : get().refreshThreads();
  }

  // Re-render a chat's thread from its persisted transcript (its settled
  // state, once a turn this client only rejoined has ended out of sight).
  async function reloadTranscript(id: string) {
    const messages = await sessionMessages(id).catch(() => undefined);
    if (messages === undefined) return;
    set((s) => withThread(s, id, () => transcriptToItems(messages)));
  }

  // The prompt a rejoined turn's retry card carries: its thread's last user
  // bubble (the transcript's last user message, rendered).
  function lastUserBubbleText(items: Item[]): string {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === "user") return it.text;
    }
    return "";
  }

  // Chats whose running turn this client did not start: it reloaded (or
  // booted) while the backend was mid-turn, so no `runTurn` promise will
  // settle them — the stream's turn-ended event does (see `ingestTurnEnded`).
  const rejoined = new Set<string>();

  // Show a chat: fetch its view and make it the visible session. A mid-turn
  // chat keeps streaming into this client (the event bridge is global), so
  // the store must also *know* it is running — the stop button, the thread
  // cache's protection of running chats, and the turn-ended settle all key
  // off `runStatus` — and must be re-handed anything the turn is parked on
  // (a question, an approval) whose event landed before this client existed.
  async function openSession(id: string, known?: SessionInfo) {
    const view = await resumeSession(id);
    // A mid-turn chat whose thread was released under the running-session
    // cap (or lost to a reload): the view can't carry its transcript (the
    // agent holds its lock), but the persisted messages read independently
    // of it. Rebuild from those and open a bubble for the rest of the reply
    // still streaming in.
    let rehydrated: Item[] | undefined;
    if (view.running && get().threads[id] === undefined) {
      const messages = await sessionMessages(id).catch(() => []);
      rehydrated = resumeMidTurn(transcriptToItems(messages));
    }
    set((s) => {
      // A mid-turn chat (`running`) keeps its live in-memory thread + info; a
      // cold history session seeds its thread and info from the transcript.
      // A cold transcript that stops mid-turn (the reply never arrived — an
      // error, out of credits, or the app closed) gets an inline retry card
      // so the chat can be continued with one click.
      let seeded: Item[] | undefined;
      if (view.running && s.threads[id] === undefined) {
        seeded = rehydrated;
      } else if (!view.running && s.threads[id] === undefined) {
        seeded = transcriptToItems(view.messages);
        if (endsMidTurn(view.messages)) {
          seeded = appendRetryPrompt(
            seeded,
            lastUserText(view.messages),
            [],
            "This chat stopped before the reply finished.",
          );
        }
      }
      const runStatus = { ...s.runStatus };
      if (runStatus[id] === "unread") delete runStatus[id]; // viewing it clears the dot
      if (view.running && runStatus[id] !== "running") {
        // The backend is mid-turn and this client holds no promise for it.
        runStatus[id] = "running";
        rejoined.add(id);
      }
      const threads = capThreadSessions(
        seeded === undefined ? s.threads : { ...s.threads, [id]: seeded },
        runStatus,
        id,
      );
      // A running chat's live info (real counters) beats the backend's
      // mid-turn placeholder; a client that never had it takes the placeholder
      // (or the info the caller already fetched for this chat).
      const fresh = known ?? view.info;
      const infos = view.running && s.infos[id] ? s.infos : { ...s.infos, [id]: fresh };
      const session = infos[id] ?? fresh;
      return {
        ...sweepCached(s, threads),
        ...tabFor(s, session),
        session,
        infos: retainCached(infos, threads, runStatus),
        runStatus,
        ...replayPending(s, id, view.pending ?? []),
      };
    });
    get().refreshHistory();
    // Opening the chat is looking at it: its "finished while you were away"
    // flag comes off — durably, so it stays off across restarts — and the
    // verdicts repaint once the mark has landed.
    void markSeenThenRefresh(id);
  }

  // The round-trips a running turn is parked on, as the backend replays them:
  // re-raise each exactly as its live event would have, so the question card
  // or approval prompt comes back for a client that missed it.
  function replayPending(s: AppState, id: string, pending: PendingRoundTrip[]): Partial<AppState> {
    let patch: Partial<AppState> = {};
    for (const p of pending) {
      if (p.session !== id) continue;
      if (p.type === "agent.question") {
        const { type: _type, ...question } = p;
        patch = { ...patch, question };
      } else if (p.type === "agent.approval_request") {
        const { type: _type, ...request } = p;
        patch = { ...patch, approvals: { ...(patch.approvals ?? s.approvals), [id]: request } };
      }
    }
    return patch;
  }

  // Monotonic stamp for snapshot refreshes. Concurrent calls race the network
  // (Home mount, window focus, turn end, finish clicks); only the newest call
  // gets to write, so an older response can never overwrite a newer one.
  let threadsFetchSeq = 0;

  // Drive one turn for `id` (a fresh send, or a retry that continues the existing
  // transcript), then either send the next queued prompt or settle the run status
  // (read if the chat is in view, unread if it finished offscreen). The turn's UI
  // (user bubble + streaming assistant bubble) must already be in the thread.
  // Chats whose background results landed while a turn ran: the running turn
  // usually drains them itself, but one that lands after its last drain is
  // delivered once the chat goes idle (see driveTurn's settle).
  const awaitingDelivery = new Set<string>();

  function driveTurn(id: string, text: string, paths: string[], how: "fresh" | "retry" | "deliver") {
    genSamples.delete(id); // each turn starts a fresh speed measurement
    lastActive.set(id, Date.now());
    set((s) => ({
      runStatus: { ...s.runStatus, [id]: "running" },
      // Clear any stale live estimate so this turn's meter starts from the
      // authoritative base (the usage event normally resets it, but be safe).
      liveTokens: { ...s.liveTokens, [id]: 0 },
    }));
    // A retry continues the failed turn's transcript in place; a fresh turn sends
    // the prompt (and any attachments) for the first time.
    let recovering = false;
    const turn = how === "retry" ? retryTurn(id) : how === "deliver" ? deliverPending(id) : runTurn(id, text, paths);
    turn
      .then((final) => set((s) => withThread(s, id, (t) => finalizeAssistant(t, final ?? ""))))
      .catch((e) => {
        // No failure is a dead end: a 401 swaps the reply for an inline
        // key-entry card, and everything else (out of credits, a provider
        // that stayed down through the agent's retries, no internet) gets a
        // retry card carrying the error — so once the user acts (adds
        // credits, switches models, gets back online) one click continues
        // the turn in place.
        const message = String(e);
        const auth = isAuthError(message);
        recovering = true;
        set((s) =>
          withThread(s, id, (thread) =>
            auth
              ? appendApiKeyPrompt(thread, text, paths)
              : appendRetryPrompt(thread, text, paths, message),
          ),
        );
      })
      .finally(() => {
        // A canvas "writing" signal that never produced a doc (or errored) must
        // not leave the panel stuck in the writing state. Also drop any leftover
        // streaming previews so nothing lingers after the turn.
        set((s) => ({
          canvasWriting: { ...s.canvasWriting, [id]: false },
          streamingTool: { ...s.streamingTool, [id]: undefined },
          streamingCanvas: { ...s.streamingCanvas, [id]: undefined },
        }));
        get().refreshHistory(); // the first turn gives a new session its title
        get().refreshTotalTokens(); // the turn bumped the all-time total
        // While a recovery card is up (missing key, out of credits), hold the
        // queue: draining it would just hit the same error. The card's action
        // retries this turn, then the queue flows.
        const next = recovering ? undefined : (get().queues[id] ?? [])[0];
        // A queued prompt's turn drains pending results on its own.
        const deliver = awaitingDelivery.delete(id) && !recovering && next === undefined;
        if (next !== undefined) {
          set((s) => ({ queues: { ...s.queues, [id]: (s.queues[id] ?? []).slice(1) } }));
          setTimeout(() => runTurnFor(id, next.text, next.attachments), 0); // let state settle first
        } else if (deliver) {
          setTimeout(() => deliverFor(id), 0);
        } else {
          settleRunStatus(id);
        }
        // The turn ended — every surface painting this thread's standing
        // (cards, tabs, the chat's status strip) needs to see it. Unconditional:
        // gating this on homeOpen once left an open chat's strip showing
        // "running" forever after the turn ended.
        void refreshThreadsAfterSeen(id);
      });
  }

  // Start a fresh turn: append its UI (user bubble + empty streaming reply), then
  // drive it. A pending retry card is dropped — the new prompt supersedes it (its
  // dangling user turn is still in the transcript for the model to answer).
  function runTurnFor(id: string, text: string, paths: string[]) {
    // The visible chat always gets its thread (created here on a first send);
    // a background chat draining its queue after its thread was evicted runs
    // headless instead of growing a stub thread holding only this turn.
    set((s) =>
      s.session?.session_id === id || s.threads[id] !== undefined
        ? { threads: { ...s.threads, [id]: startTurn(dropRetryPrompts(s.threads[id] ?? []), text, paths) } }
        : {},
    );
    driveTurn(id, text, paths, "fresh");
  }

  // Start a turn from finished background work: no user bubble, just the reply.
  function deliverFor(id: string) {
    set((s) => withThread(s, id, openReply));
    driveTurn(id, "", [], "deliver");
  }

  return {
    mode: initialMode(),
    theme: null,
    heroGame: getUi("heroGame") ?? null,
    gameDockOpen: false,
    keyStatus: null,
    refreshKeyStatus: async () => {
      // A failed read leaves the status unknown rather than guessing "no key":
      // a wrong guess would nag someone who is already signed in.
      const keyStatus = await loadKeyStatus().catch(() => null);
      set({ keyStatus });
    },
    saveApiKey: async (session, key) => {
      await configureOxenKey(session, key);
      await get().refreshKeyStatus();
    },
    session: null,
    totalTokensUsed: 0,
    totalCostUsd: null,
    sessions: [],
    projects: [],
    cloudModels: [],
    localSwitch: null,
    downloads: {},
    downloadsRev: 0,
    // Home is the application's navigation root: every project, and the way
    // into any of its chats.
    homeOpen: true,
    projectHomePath: null,
    threadsSnapshot: null,
    infos: {},
    threads: {},
    liveTokens: {},
    sessionUsage: {},
    treeUsage: {},
    tokensPerSecond: {},
    compression: {},
    runStatus: {},
    codeReview: {},
    fleets: {},
    agents: {},
    tasks: {},
    media: {},
    mediaUploads: {},
    mediaPrefs: null,
    ensureMediaPrefs: () => {
      if (get().mediaPrefs) return;
      getMediaPrefs()
        .then((prefs) => set({ mediaPrefs: prefs }))
        .catch(() => {});
    },
    mediaFocus: null,
    pendingAttachments: [],
    inspectorLive: false,
    agentView: {},
    queues: {},
    canvases: {},
    activeCanvas: {},
    canvasWriting: {},
    streamingTool: {},
    streamingCanvas: {},
    previews: {},
    previewClosed: {},
    previewErrors: {},
    workContexts: getUi("workContexts") ?? {},
    rightTab: {},
    openWorkView: (session, target, agent = false) => {
      target = availableTarget(target);
      set((s) => {
        if (agent && s.workContexts[session]?.pinned) return {};
        const previous = s.workContexts[session];
        // Hydrating a disabled package's file must keep its saved forward history.
        const context = previous && JSON.stringify(availableTarget(previous.current)) === JSON.stringify(target)
          ? previous
          : navigate(previous, target);
        const workContexts = { ...s.workContexts, [session]: context };
        setUi("workContexts", workContexts);
        return { workContexts, rightTab: { ...s.rightTab, [session]: target.view },
          ...(target.view === "editor" && (target.paths || target.path) ? { editorTabs: { ...s.editorTabs, [session]: addEditorTab(s.editorTabs[session], target.paths ?? [target.path!]) } } : {}),
          ...(target.view === "canvas" && target.id ? { activeCanvas: { ...s.activeCanvas, [session]: target.id } } : {}),
          ...(session === s.session?.session_id && target.view !== "welcome" ? revealRightColumn(s) : {}),
        };
      });
    },
    travelWorkView: (offset) => {
      const s = get(), id = s.session?.session_id; if (!id || !s.workContexts[id]) return;
      const context = travel(s.workContexts[id], offset);
      s.openWorkView(id, context.current);
      const workContexts = { ...get().workContexts, [id]: context }; setUi("workContexts", workContexts); set({workContexts});
    },
    pinWorkView: () => set((s) => {
      const id = s.session?.session_id; if (!id || !s.workContexts[id]) return {};
      const workContexts = { ...s.workContexts, [id]: { ...s.workContexts[id], pinned: !s.workContexts[id].pinned } }; setUi("workContexts", workContexts); return { workContexts };
    }),
    browserUrl: null,
    leftTab: null,
    chatTabs: parseChatTabs(getUi("chatTabs")),
    historyOpen: false,
    filesReveal: null,
    editorTabs: {},
    fsChange: null,
    snippets: {},
    gitStates: {},
    editorWrap: getUi("editorWrap") ?? false,
    dockWidths: loadDockLayout().widths,
    dockCollapsed: loadDockLayout().collapsed,
    settingsOpen: false,
    settingsPage: "connection",
    inspector: null,
    question: null,
    approvals: {},

    setMode: (mode) => {
      document.documentElement.dataset.theme = mode;
      setUi("mode", mode);
      set({ mode });
    },
    toggleMode: () => get().setMode(get().mode === "light" ? "dark" : "light"),

    setHeroGame: (name) => {
      setUi("heroGame", name);
      set({ heroGame: name });
    },
    setGameDockOpen: (open) => set({ gameDockOpen: open }),

    applyTheme: (theme) => {
      applyThemePalette(theme);
      applyThemeStyle(theme);
      set({ theme });
    },

    refreshHistory: async () => {
      try {
        // Projects derive from sessions' workspaces, so refresh both together.
        const [sessions, projects] = await Promise.all([listSessions(), listProjects()]);
        set({ sessions, projects });
        return true;
      } catch {
        /* leave the previous lists in place on a transient error */
        return false;
      }
    },

    loadSession: async () => {
      // The backend's current chat, then the same open path a tab click
      // takes — so a boot into a chat that is mid-turn (a reload while the
      // agent works) rejoins it rather than showing a blank, idle-looking one.
      const info = await sessionInfo();
      await openSession(info.session_id, info);
    },

    // Start a fresh chat. Any running chat keeps going in the background.
    startNewSession: async () => {
      const info = await newSession();
      set((s) => {
        const threads = capThreadSessions({ ...s.threads, [info.session_id]: [] }, s.runStatus, info.session_id);
        return {
          ...sweepCached(s, threads),
          ...tabFor(s, info),
          session: info,
          infos: { ...retainCached(s.infos, threads, s.runStatus), [info.session_id]: info },
        };
      });
      get().refreshHistory();
    },

    newChat: async () => {
      const s = get();
      if (s.session && isFreshChat(s, s.session.session_id)) return;
      await s.startNewSession();
    },

    resume: async (id) => {
      if (id === get().session?.session_id) return;
      await openSession(id);
    },

    renameSession: async (id, title) => {
      const name = title.trim();
      // Only a listed chat can be patched in place; a fresh one takes the
      // name once its first turn lands and the list picks it up.
      if (name) {
        set((s) => ({
          sessions: s.sessions.map((x) => (x.id === id ? { ...x, title: name } : x)),
        }));
      }
      await renameSessionIpc(id, name);
      await Promise.all([get().refreshHistory(), get().refreshThreads()]);
    },

    removeSession: async (id) => {
      await get().removeSessions([id]);
    },

    removeSessions: async (ids) => {
      // One chat or many at once: parallel deletes, one state sweep, one
      // history+threads refresh.
      await Promise.all(ids.map((id) => deleteSession(id)));
      for (const id of ids) {
        genSamples.delete(id);
        lastActive.delete(id);
        pendingTokens.delete(id);
      }
      const gone = new Set(ids);
      const current = get().session?.session_id ?? "";
      const wasCurrent = gone.has(current);
      // If the visible chat is among them, land on its nearest surviving
      // tab — the strip with the other deleted ids already gone.
      const strip = stripOf(get().chatTabs, current)?.[1] ?? [];
      const landing = wasCurrent
        ? neighbourTab(
            strip.filter((id) => id === current || !gone.has(id)),
            current,
          )
        : null;
      // Forget every per-session slice so nothing lingers for deleted chats.
      set((s) => {
        const drop = <T,>(rec: Record<string, T>) => {
          const copy = { ...rec };
          for (const id of ids) delete copy[id];
          return copy;
        };
        return {
          ...tabsPatch(withoutTabs(s.chatTabs, ids)),
          session: wasCurrent ? null : s.session,
          threads: drop(s.threads),
          infos: drop(s.infos),
          runStatus: drop(s.runStatus),
          codeReview: drop(s.codeReview),
          fleets: ids.reduce((left, id) => withoutFleetsOf(left, id), s.fleets),
          queues: drop(s.queues),
          canvases: drop(s.canvases),
          activeCanvas: drop(s.activeCanvas),
          canvasWriting: drop(s.canvasWriting),
          streamingTool: drop(s.streamingTool),
          streamingCanvas: drop(s.streamingCanvas),
          liveTokens: drop(s.liveTokens),
          sessionUsage: drop(s.sessionUsage),
          treeUsage: drop(s.treeUsage),
          tokensPerSecond: drop(s.tokensPerSecond),
          compression: drop(s.compression),
          // The backend stops the deleted chat's dev server and closes its
          // webview; drop the mirrored state so no stopped server lingers in
          // the sidebar chips or the Settings → Preview list.
          previews: drop(s.previews),
          previewClosed: drop(s.previewClosed),
          previewErrors: drop(s.previewErrors),
          rightTab: drop(s.rightTab),
          editorTabs: drop(s.editorTabs),
          snippets: drop(s.snippets),
        };
      });
      await Promise.all([get().refreshHistory(), get().refreshThreads()]);
      // The chat in view is gone: show its neighbour, or a fresh chat when it
      // was the strip's last, so the UI is never empty.
      if (wasCurrent) await (landing ? get().resume(landing) : get().startNewSession());
    },

    setHomeOpen: (homeOpen) => {
      if (homeOpen) void get().refreshThreads();
      set({ homeOpen, projectHomePath: null });
    },

    refreshThreads: async () => {
      const seq = ++threadsFetchSeq;
      try {
        const snapshot = await threadsSnapshot();
        if (seq !== threadsFetchSeq) return; // superseded by a newer refresh
        set({ threadsSnapshot: snapshot });
        // The snapshot's running set is authoritative. A rejoined turn that
        // ended before this client was listening (it finished in the gap
        // between the resume and the event bridge) has no turn-ended event
        // coming — settle it from the snapshot and show the transcript's
        // final state instead of a bubble that never closes.
        for (const id of [...rejoined]) {
          if (snapshot.running.includes(id)) continue;
          get().ingestTurnEnded({ session: id, text: "" });
          void reloadTranscript(id);
        }
      } catch {
        /* keep the previous snapshot on a transient error */
      }
    },

    finishThread: async (id) => {
      await sessionFinish(id);
      await get().refreshThreads();
    },

    reopenThread: async (id) => {
      await sessionReopen(id);
      await get().refreshThreads();
    },

    setThreadReview: async (id, status) => {
      await setReviewStatusIpc(id, status);
      await Promise.all([get().refreshThreads(), get().refreshHistory()]);
    },

    openProjectHome: (projectHomePath) => set({ homeOpen: true, projectHomePath }),

    selectProject: async (path) => {
      await setActiveProject(path);
      await get().refreshHistory();
    },

    openProjectThreads: async (workspace) => {
      // A chat can be reached before Home ever loaded the snapshot.
      if (!get().threadsSnapshot) await get().refreshThreads();
      set((s) => {
        const ids = openThreadIds(threadsFromState(s), workspace);
        const next = ids.reduce((tabs, id) => withTab(tabs, workspace, id), s.chatTabs);
        return next === s.chatTabs ? {} : tabsPatch(next);
      });
    },

    prepareProject: async (path, model) => {
      await setActiveProject(path);
      // The project's loose ends first, so the fresh chat lands beside them.
      await get().openProjectThreads(path);
      if (model?.local) {
        await get().switchToLocalModel(model.id);
      } else {
        if (model) await selectCloudModelForNewChats(model.id);
        await get().startNewSession();
      }
    },

    enterProject: async (path) => {
      await get().prepareProject(path);
      get().setHomeOpen(false);
    },

    removeProject: async (path) => {
      await deleteProject(path);
      // Its tabs go with it: a strip for a project that no longer exists
      // would only come back as dead tabs if the folder were re-added.
      set((s) => {
        const next = withoutStrip(s.chatTabs, path);
        return next === s.chatTabs ? {} : tabsPatch(next);
      });
      // Both lists must forget it: the projects (cards) and the thread
      // snapshot (removed workspaces drop out of it).
      await Promise.all([get().refreshHistory(), get().refreshThreads()]);
    },

    adoptSession: (info) =>
      set((s) => {
        const threads = capThreadSessions({ ...s.threads, [info.session_id]: [] }, s.runStatus, info.session_id);
        return {
          ...sweepCached(s, threads),
          ...tabFor(s, info),
          session: info,
          infos: { ...retainCached(s.infos, threads, s.runStatus), [info.session_id]: info },
        };
      }),

    loadCloudModels: async () => {
      try {
        set({ cloudModels: await listCloudModels() });
      } catch {
        /* leave the previous catalog in place on a transient error */
      }
    },

    changeCompressionMode: async (mode) => {
      const info = await setCompressionMode(mode);
      // In-place switch on the same session: refresh its info (which carries
      // the new mode) and leave the thread untouched.
      set((s) => ({
        session: info,
        infos: { ...s.infos, [info.session_id]: info },
      }));
    },

    changeModel: async (model) => {
      const info = await setModel(model);
      // In-place swap: the backend kept the same session, so keep the thread and
      // only update the model/info for the current chat.
      set((s) => ({
        session: info,
        infos: { ...s.infos, [info.session_id]: info },
        threads: { ...s.threads, [info.session_id]: s.threads[info.session_id] ?? [] },
      }));
      get().loadCloudModels(); // refresh the selected flag
      get().refreshHistory(); // the history list shows each chat's model
    },

    switchToLocalModel: async (id) => {
      set({ localSwitch: { model: id, phase: "starting", startedAt: Date.now() } });
      try {
        const info = await useLocalModel(id); // a local model starts a fresh session
        get().adoptSession(info);
        get().loadCloudModels();
        get().refreshHistory();
      } finally {
        set({ localSwitch: null });
      }
    },

    startDownload: (model) => {
      const existing = get().downloads[model.id];
      if (existing && existing.status !== "error") return; // already in flight
      set((s) => ({
        downloads: {
          ...s.downloads,
          [model.id]: {
            model,
            downloaded: 0,
            total: model.size_bytes > 0 ? model.size_bytes : null,
            fraction: 0,
            status: "downloading",
            startedAt: Date.now(),
          },
        },
      }));
      // The invoke lives here — not in a component — so it survives the settings
      // page unmounting; the row clears (or turns into an error) when it settles.
      downloadModelIpc(model)
        .then(() => {
          set((s) => {
            const { [model.id]: _done, ...rest } = s.downloads;
            return { downloads: rest, downloadsRev: s.downloadsRev + 1 };
          });
        })
        .catch((e) => {
          set((s) => {
            const entry = s.downloads[model.id];
            if (!entry) return {};
            // A stopped download rejects its invoke; that's the expected end of
            // a cancel, not a failure to report.
            if (entry.status === "cancelling") {
              const { [model.id]: _gone, ...rest } = s.downloads;
              return { downloads: rest };
            }
            return {
              downloads: {
                ...s.downloads,
                [model.id]: { ...entry, status: "error", error: String(e) },
              },
            };
          });
        });
    },

    stopDownload: (id) => {
      const entry = get().downloads[id];
      if (!entry || entry.status !== "downloading") return;
      set((s) => ({
        downloads: { ...s.downloads, [id]: { ...s.downloads[id], status: "cancelling" } },
      }));
      // Fire-and-forget, like cancelTurn: the pending downloadModel invoke is
      // what observes the cancel and clears the row.
      void cancelDownloadIpc(id).catch(() => {});
    },

    dismissDownload: (id) =>
      set((s) => {
        const { [id]: _gone, ...rest } = s.downloads;
        return { downloads: rest };
      }),

    ingestDownloadProgress: (p) =>
      set((s) => {
        const entry = s.downloads[p.id];
        if (!entry || entry.status !== "downloading") return {};
        return {
          downloads: {
            ...s.downloads,
            [p.id]: {
              ...entry,
              downloaded: p.downloaded,
              total: p.total ?? entry.total,
              fraction: p.fraction ?? entry.fraction,
            },
          },
        };
      }),

    setLocalStatus: (s) =>
      set((st) => {
        // "ready"/"error" means the load is over — clear the inline state.
        if (s.phase === "ready" || s.phase === "error")
          return st.localSwitch ? { localSwitch: null } : {};
        // Create-or-update: a load the picker didn't start (the active local
        // model's server died and is being restarted for a new chat) must
        // surface the same way an explicit switch does. Nothing loads at
        // launch — a local model runs only once picked in this run.
        return {
          localSwitch: st.localSwitch
            ? { ...st.localSwitch, model: s.model, phase: s.phase }
            : { model: s.model, phase: s.phase, startedAt: Date.now() },
        };
      }),

    send: (text, attachments = []) => {
      const id = get().session?.session_id;
      if (!id) return;
      // Bake staged code snippets into the prompt now (not at turn time), so a
      // queued prompt carries exactly the context that was on screen when it
      // was written, and the chips clear the moment they're consumed.
      const staged = get().snippets[id] ?? [];
      if (staged.length) {
        text = withSnippetContext(text, staged);
        set((s) => ({ snippets: { ...s.snippets, [id]: [] } }));
      }
      if (get().runStatus[id] === "running") {
        const prompt = { text, attachments };
        set((s) => {
          const q = s.queues[id] ?? [];
          // Bounded backstop: once the queue is saturated, ignore further sends
          // rather than letting it grow unbounded in memory.
          if (q.length >= MAX_QUEUE) return {};
          return { queues: { ...s.queues, [id]: [...q, prompt] } };
        });
        return;
      }
      runTurnFor(id, text, attachments);
    },

    stop: () => {
      const id = get().session?.session_id;
      if (!id) return;
      // Tell the backend to cancel; the in-flight runTurn promise then resolves
      // with its partial reply and runTurnFor's normal completion path clears the
      // "running" status. Fire-and-forget — a failed cancel just leaves it running.
      // A running code review registers under the same token, so this stops it too.
      void cancelTurn(id).catch(() => {});
    },

    startCodeReview: (baseBranch) => {
      const id = get().session?.session_id;
      if (!id) return;
      if (get().runStatus[id] === "running") return; // never interleave with a turn
      set((s) => ({
        runStatus: { ...s.runStatus, [id]: "running" },
        codeReview: {
          ...s.codeReview,
          [id]: { step: "resolving the diff", index: 0, total: 0, activity: "" },
        },
      }));
      runCodeReviewIpc(id, baseBranch)
        .then((res) =>
          set((s) => {
            const thread = s.threads[id] ?? [];
            // Success: the exchange is already persisted backend-side; mirror it
            // into the live thread as a settled user + assistant pair.
            if (res.status === "ok") {
              return {
                threads: {
                  ...s.threads,
                  [id]: finalizeAssistant(startTurn(thread, res.user), res.assistant),
                },
              };
            }
            const note =
              res.status === "nothing"
                ? "Nothing to review — the workspace has no changes."
                : "Code review stopped.";
            return { threads: { ...s.threads, [id]: appendNotice(thread, note) } };
          }),
        )
        .catch((e) =>
          set((s) => ({
            threads: {
              ...s.threads,
              [id]: appendNotice(s.threads[id] ?? [], `Code review failed: ${e}`),
            },
          })),
        )
        .finally(() => {
          // The review is over however it ended — clear both the progress card
          // and any fan-out lanes panel. Clearing fleets here (not just on the
          // backend's fleet://completed) is what closes the panel when a
          // fan-out step was cancelled or every lane failed: those paths never
          // emit StepCompleted, so no fleet://completed arrives.
          set((s) => {
            const codeReview = { ...s.codeReview };
            delete codeReview[id];
            const fleets = withoutFleetsOf(s.fleets, id);
            return { codeReview, fleets };
          });
          get().refreshHistory();
          get().refreshTotalTokens();
          // Drain anything queued while the review ran, else settle the status
          // (read if the chat is in view, unread if it finished offscreen).
          const next = (get().queues[id] ?? [])[0];
          if (next !== undefined) {
            set((s) => ({ queues: { ...s.queues, [id]: (s.queues[id] ?? []).slice(1) } }));
            setTimeout(() => runTurnFor(id, next.text, next.attachments), 0);
          } else {
            settleRunStatus(id);
            void refreshThreadsAfterSeen(id);
          }
        });
    },

    addNotice: (text) =>
      set((s) => {
        const id = s.session?.session_id;
        return id ? { threads: { ...s.threads, [id]: appendNotice(s.threads[id] ?? [], text) } } : {};
      }),

    ingestCodeReviewProgress: (e) =>
      set((s) => {
        if (!s.codeReview[e.session]) return {}; // no card = not our review
        return {
          codeReview: {
            ...s.codeReview,
            [e.session]: { step: e.step, index: e.index, total: e.total, activity: "" },
          },
        };
      }),

    ingestCodeReviewActivity: (session, text, replace) =>
      set((s) => {
        const cur = s.codeReview[session];
        if (!cur) return {};
        // A one-line rolling tail: newlines flatten, only the end is kept.
        const joined = replace ? text : (cur.activity + text).replace(/\s+/g, " ");
        const activity = tailChars(joined, LANE_ACTIVITY_CAP);
        return { codeReview: { ...s.codeReview, [session]: { ...cur, activity } } };
      }),

    ingestFleetStarted: (e) =>
      set((s) => ({
        fleets: {
          ...s.fleets,
          [e.fleet]: {
            session: e.session,
            source: e.source,
            call: e.call,
            focused: null,
            lanes: e.agents.map((name) => ({
              name,
              id: "",
              status: "queued" as const,
              activity: "",
              tail: "",
              tokens: 0,
            })),
          },
        },
      })),

    ingestFleetAgent: (e) =>
      set((s) => {
        const fleet = s.fleets[e.fleet];
        const lane = fleet?.lanes[e.agent];
        if (!fleet || !lane) return {};
        const updated: FleetLane =
          e.phase === "started"
            ? { ...lane, status: "running", id: e.lane }
            : {
                ...lane,
                id: e.lane || lane.id,
                status: e.phase,
                tokens: e.tokens,
                activity: e.summary || lane.activity,
              };
        const lanes = fleet.lanes.map((l, i) => (i === e.agent ? updated : l));
        return { fleets: { ...s.fleets, [e.fleet]: { ...fleet, lanes } } };
      }),

    ingestFleetActivity: (e) =>
      set((s) => {
        const fleet = s.fleets[e.fleet];
        const lane = fleet?.lanes[e.agent];
        if (!fleet || !lane) return {};
        let updated: FleetLane;
        if (e.kind === "token") {
          updated = {
            ...lane,
            activity: tailChars((lane.activity + e.text).replace(/\s+/g, " "), LANE_ACTIVITY_CAP),
            tail: tailChars(lane.tail + e.text, LANE_TAIL_CAP),
          };
        } else if (e.kind === "tool") {
          updated = {
            ...lane,
            activity: `⚙ ${e.text}…`,
            tail: tailChars(`${lane.tail}\n◆ ${e.text}…\n`, LANE_TAIL_CAP),
          };
        } else if (e.kind === "note") {
          updated = {
            ...lane,
            activity: `ℹ ${e.text}`,
            tail: tailChars(`${lane.tail}\nℹ ${e.text}\n`, LANE_TAIL_CAP),
          };
        } else {
          updated = { ...lane, tokens: e.tokens ?? lane.tokens };
        }
        const lanes = fleet.lanes.map((l, i) => (i === e.agent ? updated : l));
        return { fleets: { ...s.fleets, [e.fleet]: { ...fleet, lanes } } };
      }),

    ingestFleetBudget: (e) => {
      get().refreshTreeUsage(e.session);
      set((s) => {
        const fleet = s.fleets[e.fleet];
        if (!fleet) return {};
        const budget = {
          tokens: e.tokens,
          max_tokens: e.max_tokens,
          spawns: e.spawns,
          max_spawns: e.max_spawns,
        };
        return { fleets: { ...s.fleets, [e.fleet]: { ...fleet, budget } } };
      });
    },

    ingestFleetCompleted: (session, id) => {
      set((s) => {
        if (!s.fleets[id]) return {};
        const fleets = { ...s.fleets };
        fleets[id] = { ...fleets[id]!, finished: true };
        const finished = Object.entries(fleets).filter(([, f]) => f?.session === session && f.finished);
        for (const [key] of finished.slice(0, -20)) delete fleets[key];
        return { fleets };
      });
      // The lanes just settled: their records are the hub's rows now.
      void get().refreshAgents(session);
    },

    refreshAgents: async (session) => {
      try {
        const rows = await listAgents(session);
        set((s) => ({ agents: { ...s.agents, [session]: rows } }));
      } catch {
        // A hub that fails to load keeps whatever it showed.
      }
    },

    setFleetFocus: (id, index) =>
      set((s) => {
        const fleet = s.fleets[id];
        if (!fleet) return {};
        const focused = index !== null && index < fleet.lanes.length ? index : null;
        return { fleets: { ...s.fleets, [id]: { ...fleet, focused } } };
      }),

    stopFleet: (session, fleet) => {
      // Best effort: a fleet that already ended (false) or an IPC hiccup
      // leaves the panel to the backend's completion event either way.
      return cancelFleet(session, fleet);
    },

    stopLane: (session, lane) => {
      return cancelAgent(session, lane);
    },

    steerLane: (session, lane, text) => {
      return interjectAgent(session, lane, text);
    },

    watchLane: (lane) => set({ inspector: { sessionId: lane, review: null }, inspectorLive: true }),

    openAgent: (session, lane) => set((s) => ({ agentView: { ...s.agentView, [session]: lane } })),

    closeAgent: (session) => set((s) => ({ agentView: { ...s.agentView, [session]: null } })),

    ingestTasksChanged: (e) => set((s) => ({ tasks: { ...s.tasks, [e.session]: e.tasks } })),

    ingestDeliveryReady: (session) => {
      const s = get();
      if (s.runStatus[session] === "running" || (s.queues[session] ?? []).length > 0) {
        awaitingDelivery.add(session);
        return;
      }
      deliverFor(session);
    },

    refreshTasks: async (session) => {
      try {
        const rows = await listTasks(session);
        // An initial fill only: a `tasks://changed` event that landed while
        // the fetch was in flight is fresher than the fetch.
        set((s) => (s.tasks[session] === undefined ? { tasks: { ...s.tasks, [session]: rows } } : {}));
      } catch {
        // Keep what the last event said.
      }
    },

    killTask: (session, id) => {
      void killBackgroundTask(session, id).catch(() => {});
    },

    ingestMediaChanged: (e) =>
      set((s) => ({
        media: { ...s.media, [e.root]: e.items },
        mediaUploads: { ...s.mediaUploads, [e.root]: e.uploads ?? [] },
      })),

    refreshMedia: async (root) => {
      try {
        const items = await listMedia(root);
        // An initial fill only: a `media://changed` event that landed while
        // the fetch was in flight is fresher than the fetch.
        set((s) => (s.media[root] === undefined ? { media: { ...s.media, [root]: items } } : {}));
      } catch {
        // Keep what the last event said.
      }
    },

    cancelMedia: (id) => {
      void cancelMediaIpc(id).catch(() => {});
    },

    openGallery: (itemId) => {
      set({ mediaFocus: itemId ?? null });
      const session=get().session?.session_id;
      if(session)get().openWorkView(session,{view:"gallery",id:itemId});
    },

    clearMediaFocus: () => set({ mediaFocus: null }),

    revealInFiles: (path) => {
      get().setLeftTab("files");
      set((s) => ({ filesReveal: { path, tick: (s.filesReveal?.tick ?? 0) + 1 } }));
    },

    clearFilesReveal: () => set({ filesReveal: null }),

    stageAttachment: (path) =>
      set((s) =>
        s.pendingAttachments.includes(path)
          ? {}
          : { pendingAttachments: [...s.pendingAttachments, path] },
      ),

    takePendingAttachments: () => {
      const paths = get().pendingAttachments;
      if (paths.length) set({ pendingAttachments: [] });
      return paths;
    },

    submitApiKey: async (session, itemId, key) => {
      // Don't drive a retry into a chat that's already busy. A code review
      // registers the session as "running" and holds its agent lock and its
      // cancel-map slot for the whole run; letting a key submission start a
      // turn underneath it would clobber that slot (Stop would then target the
      // wrong work, and the review's cleanup would delete the turn's token).
      // The key still saves for next time via configureOxenKey below only if
      // we proceed — so bail before any state change.
      if (get().runStatus[session] === "running") return;
      const item = (get().threads[session] ?? []).find((it) => it.id === itemId);
      if (!item || item.kind !== "apikey") return;
      // Save + authenticate the running agent first; if this throws, the card
      // stays put so the form can show the error and let the user try again.
      await configureOxenKey(session, key);
      void get().refreshKeyStatus();
      // Retire the card, open a fresh reply bubble, and retry the failed turn
      // (which continues the existing transcript — no duplicate user message).
      set((s) => ({
        threads: { ...s.threads, [session]: resolveRecoveryPrompt(s.threads[session] ?? [], itemId) },
      }));
      driveTurn(session, item.text, item.attachments, "retry");
    },

    retryBrokenTurn: (session, itemId) => {
      if (get().runStatus[session] === "running") return; // a turn is already in flight
      const item = (get().threads[session] ?? []).find((it) => it.id === itemId);
      if (!item || item.kind !== "retry") return;
      set((s) => ({
        threads: { ...s.threads, [session]: resolveRecoveryPrompt(s.threads[session] ?? [], itemId) },
      }));
      driveTurn(session, item.text, item.attachments, "retry");
    },

    setQueue: (items) =>
      set((s) => {
        const id = s.session?.session_id;
        return id ? { queues: { ...s.queues, [id]: reconcileQueueTexts(s.queues[id], items) } } : {};
      }),

    ingestToken: (session, token) => {
      if (get().threads[session] === undefined) return;
      const est = token.length / CHARS_PER_TOKEN;
      // Measure decode speed over the current streaming burst: start a fresh
      // sample after a gap (e.g. a tool call), so the rate isn't dragged down by
      // idle time between model calls.
      const now = Date.now();
      let smp = genSamples.get(session);
      if (!smp || now - smp.last > BURST_GAP_MS) smp = { start: now, tokens: 0, last: now };
      smp.tokens += est;
      smp.last = now;
      genSamples.set(session, smp);
      const secs = (now - smp.start) / 1000;
      // Need a small window before the rate is meaningful; otherwise keep the last.
      const tps = secs >= 0.3 ? smp.tokens / secs : null;
      lastActive.set(session, now);
      // Buffer, don't set: the flush (on the timer, or ahead of the next store
      // mutation, whichever comes first) lands the whole burst in one copy.
      const p = pendingTokens.get(session) ?? { text: "", est: 0, tps: null };
      p.text += token;
      p.est += est;
      if (tps !== null) p.tps = tps;
      pendingTokens.set(session, p);
      if (tokenFlushTimer == null) tokenFlushTimer = window.setTimeout(flushTokens, TOKEN_FLUSH_MS);
    },

    flushTokens,

    ingestTool: (e) => {
      lastActive.set(e.session, Date.now());
      // One set() per event — this is the streaming hot path, and every set()
      // re-renders every subscriber.
      set((s) => {
        const update: Partial<AppState> = {};
        if (s.threads[e.session] !== undefined) {
          // The call's args are fully assembled now (the real tool chip takes
          // over), so drop the streaming file preview. Canvas keeps its
          // provisional doc until the committed version lands via ingestCanvas.
          update.threads = {
            ...s.threads,
            [e.session]:
              e.phase === "start"
                ? toolStart(s.threads[e.session], e.name, e.detail, Date.now(), e.call_id)
                : toolEnd(s.threads[e.session], e.name, e.detail, Date.now(), e.call_id),
          };
          update.streamingTool = { ...s.streamingTool, [e.session]: undefined };
        }
        return update;
      });
    },

    ingestToolDelta: (e) =>
      set((s) => {
        const prev = s.streamingTool[e.session];
        const combined = prev && prev.name === e.name ? prev.args + e.delta : e.delta;
        const args = combined.slice(0, MAX_STREAMING_TOOL_ARGS);
        const update: Partial<AppState> = {
          streamingTool: { ...s.streamingTool, [e.session]: { name: e.name, args } },
        };
        // Canvas streams into the side panel: build a provisional doc so the
        // panel shows the document forming before the committed version lands.
        if (e.name === "canvas") {
          const doc = partialCanvasDoc(args);
          if (doc) update.streamingCanvas = { ...s.streamingCanvas, [e.session]: doc };
        }
        return update;
      }),

    ingestUsage: (e) => {
      get().refreshTreeUsage(e.session);
      set((s) => {
        const info = s.infos[e.session];
        if (!info) return {};
        const updated = {
          ...info,
          tokens_used: e.tokens_used,
          context_tokens: e.context_tokens,
          context_window: e.context_window,
        };
        return {
          infos: { ...s.infos, [e.session]: updated },
          session: s.session?.session_id === e.session ? updated : s.session,
          // This event carries the exact count up to the current model call, so
          // drop the live streaming estimate to avoid double-counting.
          liveTokens: { ...s.liveTokens, [e.session]: 0 },
          sessionUsage: {
            ...s.sessionUsage,
            [e.session]: { prompt: e.prompt_tokens_used, completion: e.completion_tokens_used },
          },
        };
      });
    },

    ingestCompression: (e) =>
      set((s) => ({
        compression: {
          ...s.compression,
          [e.session]: { mode: e.mode, tokensSaved: e.total_saved_tokens },
        },
      })),

    ingestCompacted: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        return {
          threads: {
            ...s.threads,
            [e.session]: appendNotice(s.threads[e.session], `Compacted context — ${e.detail}`),
          },
        };
      }),

    ingestToolProgress: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        return {
          threads: {
            ...s.threads,
            [e.session]: toolProgress(s.threads[e.session], e.name, e.chunk, e.call_id),
          },
        };
      }),

    ingestNotice: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        return {
          threads: { ...s.threads, [e.session]: appendNotice(s.threads[e.session], e.text) },
        };
      }),

    ingestRetry: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        const wait = Math.max(1, Math.ceil(e.delay_ms / 1000));
        // A fallback restarts the attempt budget on a different model, so
        // "attempt 5 of 4" would read as a bug rather than a recovery.
        const notice = e.switching_to
          ? `Model call failed (${e.error}) — ${e.max_attempts} attempts spent, continuing on ${e.switching_to}`
          : `Model call failed (${e.error}) — retrying in ${wait}s (attempt ${e.attempt + 1} of ${e.max_attempts})`;
        // The notice stays a one-liner; the structured failure rides along so
        // clicking it opens the provider's actual response, not just the line.
        const error: ModelErrorDetail = {
          at: Date.now(),
          error: e.error,
          model: e.model || undefined,
          endpoint: e.endpoint || undefined,
          status: e.status,
          detail: e.detail,
          attempt: e.attempt,
          maxAttempts: e.max_attempts,
          next: e.switching_to
            ? { kind: "switch", model: e.switching_to }
            : { kind: "retry", delayMs: e.delay_ms },
        };
        return {
          threads: {
            ...s.threads,
            [e.session]: appendNotice(s.threads[e.session], notice, error),
          },
        };
      }),

    refreshTreeUsage: (session) => {
      const pending = treeUsageTimers.get(session);
      if (pending != null) window.clearTimeout(pending);
      treeUsageTimers.set(
        session,
        window.setTimeout(() => {
          treeUsageTimers.delete(session);
          sessionTreeUsage(session)
            .then((usage) => {
              if (!usage) return;
              set((s) => ({
                treeUsage: {
                  ...s.treeUsage,
                  [session]: {
                    tokens: usage.prompt_tokens + usage.completion_tokens,
                    cost: usage.total_cost_usd,
                    unpriced: usage.has_unpriced_usage,
                  },
                },
              }));
            })
            .catch(() => {
              /* keep the previous figure on a transient error */
            });
        }, TREE_USAGE_DEBOUNCE_MS),
      );
    },

    refreshTotalTokens: async () => {
      try {
        set({ totalTokensUsed: await totalTokensUsed() });
      } catch {
        /* leave the previous total in place on a transient error */
      }
      // Cost is a separate best-effort call (it hits the network for pricing);
      // keep it out of the token refresh's try so a pricing hiccup doesn't stop
      // the token count from updating.
      try {
        set({ totalCostUsd: await totalCostUsd() });
      } catch {
        /* leave the previous cost in place on a transient error */
      }
    },

    ingestCanvas: ({ session, ...doc }) =>
      set((s) => {
        const list = s.canvases[session] ?? [];
        const i = list.findIndex((d) => d.id === doc.id);
        // Update in place if the id exists, else append (capped). Opening it
        // focuses the panel on this doc for that session and clears "writing".
        const next = capCanvases(i >= 0 ? list.map((d, j) => (j === i ? doc : d)) : [...list, doc]);
        return {
          canvases: { ...s.canvases, [session]: next },
          activeCanvas: { ...s.activeCanvas, [session]: doc.id },
          canvasWriting: { ...s.canvasWriting, [session]: false },
          // A fresh document takes the panel over from the live preview.
          rightTab: { ...s.rightTab, [session]: "canvas" as const },
          // The committed doc supersedes both streaming buffers; release the raw
          // arg string too so a large doc isn't held twice once it lands.
          streamingCanvas: { ...s.streamingCanvas, [session]: undefined },
          streamingTool: { ...s.streamingTool, [session]: undefined },
        };
      }),

    ingestFsChange: (e) =>
      set((s) => ({ fsChange: { ...e, tick: (s.fsChange?.tick ?? 0) + 1 } })),

    ingestOpenFile: (e) =>
      set((s) => {
        if (!e.paths.length) return {};
        return {
          editorTabs: { ...s.editorTabs, [e.session]: addEditorTab(s.editorTabs[e.session], e.paths) },
          // Front the editor tab for that session (mirrors ingestCanvas: no
          // global side effects, so a background chat can't grab the UI).
          rightTab: { ...s.rightTab, [e.session]: "editor" as const },
        };
      }),

    setActiveCanvas: (id) =>
      set((s) => {
        const cur = s.session?.session_id;
        if (!cur) return {};
        return {
          activeCanvas: { ...s.activeCanvas, [cur]: id },
          // Opening a document must bring it to the front, even when the
          // preview currently owns the panel — otherwise the click is dead.
          ...(id ? { rightTab: { ...s.rightTab, [cur]: "canvas" as const } } : {}),
        };
      }),

    openCanvasDoc: (doc) =>
      set((s) => {
        const session = s.session?.session_id;
        if (!session) return {};
        const list = s.canvases[session] ?? [];
        const i = list.findIndex((d) => d.id === doc.id);
        const next = capCanvases(i >= 0 ? list.map((d, j) => (j === i ? doc : d)) : [...list, doc]);
        return {
          canvases: { ...s.canvases, [session]: next },
          activeCanvas: { ...s.activeCanvas, [session]: doc.id },
          rightTab: { ...s.rightTab, [session]: "canvas" as const },
        };
      }),

    setCanvasWriting: (session, writing) =>
      set((s) => ({
        canvasWriting: { ...s.canvasWriting, [session]: writing },
        // The panel follows the document the moment the model starts writing.
        ...(writing ? { rightTab: { ...s.rightTab, [session]: "canvas" as const } } : {}),
      })),

    ingestPreviewStatus: ({ session, ...status }) =>
      set((s) => {
        const was = s.previews[session]?.phase;
        // Only a *transition* into ready opens the pane. Re-firing on every
        // ready (a restart, an auto-verify cycle) would yank the panel away
        // from a canvas the model is mid-write on, and would keep reopening a
        // pane the user deliberately closed.
        const cameUp = status.phase === "ready" && was !== "ready";
        if (!cameUp) {
          return { previews: { ...s.previews, [session]: status } };
        }
        return {
          previews: { ...s.previews, [session]: status },
          previewClosed: { ...s.previewClosed, [session]: false },
          // A fresh page retires any stale error banner.
          previewErrors: { ...s.previewErrors, [session]: undefined },
          // Don't steal the panel from a document being written right now.
          rightTab: s.canvasWriting[session]
            ? s.rightTab
            : { ...s.rightTab, [session]: "preview" as const },
        };
      }),

    // An empty text means the page (re)loaded: whatever it complained about
    // belongs to a document that no longer exists — and the reload may well
    // have been the fix, so the banner must go.
    ingestPreviewConsole: ({ session, text }) =>
      set((s) => ({ previewErrors: { ...s.previewErrors, [session]: text || undefined } })),

    resolvePreviewError: (session, fix) => {
      const error = get().previewErrors[session];
      set((s) => ({ previewErrors: { ...s.previewErrors, [session]: undefined } }));
      // Only send when it's still the chat on screen — a prompt belongs to the
      // conversation the user was looking at when they clicked.
      if (fix && error && get().session?.session_id === session) {
        get().send(
          `The app in the live preview hit a JavaScript error:\n\n${error}\n\nFind the cause and fix it, then verify the fix in the preview.`,
        );
      }
    },

    syncPreview: async (session) => {
      // A cold-mount sync must never overwrite a live event that landed while
      // the round trip was in flight (it would flash the pane back to an old
      // phase, or make it vanish).
      const before = get().previews[session];
      try {
        const status = await previewStatus(session);
        set((s) =>
          s.previews[session] === before
            ? { previews: { ...s.previews, [session]: status ?? undefined } }
            : {},
        );
      } catch {
        /* preview status is best-effort */
      }
    },

    closePreview: () =>
      set((s) => {
        const cur = s.session?.session_id;
        return cur ? { previewClosed: { ...s.previewClosed, [cur]: true } } : {};
      }),

    openBrowser: (url) => {
      set({ browserUrl: url });
      // A clicked link means "show me this page" — bring the pane to the
      // front even if another tab (or a collapsed column) is hiding it.
      get().setRightTab("browser");
    },

    closeBrowser: () => { const s=get(); set({browserUrl:null}); if(s.session) s.openWorkView(s.session.session_id,{view:"welcome"}); },

    setDockWidth: (side, width) =>
      set((s) => {
        const dockWidths = { ...s.dockWidths, [side]: width };
        saveDockLayout({ widths: dockWidths, collapsed: s.dockCollapsed });
        return { dockWidths };
      }),

    setDockCollapsed: (side, collapsed) =>
      set((s) => {
        const dockCollapsed = { ...s.dockCollapsed, [side]: collapsed };
        saveDockLayout({ widths: s.dockWidths, collapsed: dockCollapsed });
        return { dockCollapsed };
      }),

    toggleDock: (side) => get().setDockCollapsed(side, !get().dockCollapsed[side]),

    setRightTab: (tab) => {
      const s=get(), id=s.session?.session_id; if(!id)return;
      const pane=s.editorTabs[id], paths=pane?.tabs[pane.active];
      const target: ViewTarget={view:tab};
      if(tab==="editor" && paths?.length) { target.paths=paths; target.path=paths.length===1 ? paths[0]:undefined; target.view=target.path ? resolveView(target.path):tab; }
      if(tab==="browser")target.url=s.browserUrl ?? undefined;
      if(tab==="canvas")target.id=s.activeCanvas[id] ?? undefined;
      s.openWorkView(id,target);
      if(tab==="preview")set({previewClosed:{...s.previewClosed,[id]:false}});
    },

    setLeftTab: (id) => {
      // Picking a dock means "show me this" — expand a collapsed column first.
      if (get().dockCollapsed.left) get().setDockCollapsed("left", false);
      set({ leftTab: id });
    },

    closeTab: async (id) => {
      const s = get();
      const strip = stripOf(s.chatTabs, id);
      if (!strip) return;
      const ids = strip[1];
      const current = s.session?.session_id;
      // The only tab, and an untouched fresh chat: closing it would just mint
      // another empty one in its place. Stay put.
      if (id === current && ids.length === 1 && isFreshChat(s, id)) return;
      const landing = neighbourTab(ids, id);
      set((s) => tabsPatch(withoutTabs(s.chatTabs, [id])));
      if (id !== current) return;
      // A neighbour that won't open (deleted behind our back) must not leave
      // the closed chat on screen with no tab: fall through to a fresh one.
      if (landing) {
        try {
          await get().resume(landing);
          return;
        } catch {
          set((s) => tabsPatch(withoutTabs(s.chatTabs, [landing])));
        }
      }
      await get().startNewSession();
    },

    closeOtherTabs: async (id) => {
      const strip = stripOf(get().chatTabs, id);
      if (!strip) return;
      set((s) => tabsPatch({ ...s.chatTabs, [strip[0]]: [id] }));
      if (get().session?.session_id !== id) await get().resume(id);
    },

    closeTabsRight: async (id) => {
      const strip = stripOf(get().chatTabs, id);
      if (!strip) return;
      const [workspace, ids] = strip;
      const kept = ids.slice(0, ids.indexOf(id) + 1);
      if (kept.length === ids.length) return;
      set((s) => tabsPatch({ ...s.chatTabs, [workspace]: kept }));
      const current = get().session?.session_id;
      if (current && !kept.includes(current)) await get().resume(id);
    },

    moveTab: (id, before) =>
      set((s) => {
        const next = movedTab(s.chatTabs, id, before);
        return next === s.chatTabs ? {} : tabsPatch(next);
      }),

    pruneTabs: () =>
      set((s) => {
        const known = new Set(s.sessions.map((x) => x.id));
        if (s.session) known.add(s.session.session_id);
        const stale = Object.values(s.chatTabs)
          .flat()
          .filter((id) => !known.has(id));
        return stale.length ? tabsPatch(withoutTabs(s.chatTabs, stale)) : {};
      }),

    setHistoryOpen: (historyOpen) => set({ historyOpen }),

    refreshGitStatus: async (root) => {
      try {
        const states = await gitStatus(root);
        set((s) => ({ gitStates: { ...s.gitStates, [root]: states } }));
      } catch {
        /* leave whatever we last knew */
      }
    },

    toggleEditorWrap: () => {
      const wrap = !get().editorWrap;
      setUi("editorWrap", wrap);
      set({ editorWrap: wrap });
    },

    openInViewer: (paths) => {
      const id = get().session?.session_id;
      if (!id || paths.length === 0) return;
      set((s) => ({ editorTabs: { ...s.editorTabs, [id]: addEditorTab(s.editorTabs[id], paths) } }));
      // Opening a file means "show me this" — bring the pane to the front.
      get().setRightTab("editor");
    },

    activateEditorTab: (index) =>
      set((s) => {
        const id = s.session?.session_id;
        const pane = id ? s.editorTabs[id] : undefined;
        if (!id || !pane || index < 0 || index >= pane.tabs.length) return {};
        return { editorTabs: { ...s.editorTabs, [id]: { ...pane, active: index } } };
      }),

    closeEditorTab: (index) =>
      set((s) => {
        const id = s.session?.session_id;
        const pane = id ? s.editorTabs[id] : undefined;
        if (!id || !pane || index < 0 || index >= pane.tabs.length) return {};
        const tabs = pane.tabs.filter((_, i) => i !== index);
        const editorTabs = { ...s.editorTabs };
        if (!tabs.length) {
          delete editorTabs[id];
        } else {
          // Closing left of the active tab shifts it; closing the active tab
          // falls through to its right neighbor (clamped at the end).
          const active = Math.min(index < pane.active ? pane.active - 1 : pane.active, tabs.length - 1);
          editorTabs[id] = { tabs, active };
        }
        return { editorTabs };
      }),

    closeViewer: () =>
      set((s) => {
        const id = s.session?.session_id;
        if (!id) return {};
        const editorTabs = { ...s.editorTabs };
        delete editorTabs[id];
        return { editorTabs };
      }),

    addSnippet: (snippet) =>
      set((s) => {
        const id = s.session?.session_id;
        if (!id) return {};
        return { snippets: { ...s.snippets, [id]: [...(s.snippets[id] ?? []), snippet] } };
      }),

    removeSnippet: (index) =>
      set((s) => {
        const id = s.session?.session_id;
        if (!id) return {};
        const staged = (s.snippets[id] ?? []).filter((_, i) => i !== index);
        return { snippets: { ...s.snippets, [id]: staged } };
      }),

    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
    openSettings: (page) =>
      set(page ? { settingsOpen: true, settingsPage: page } : { settingsOpen: true }),
    setSettingsPage: (settingsPage) => set({ settingsPage }),

    openInspector: (sessionId) =>
      set({ inspector: { sessionId, review: null }, inspectorLive: false }),
    openReview: (queue, index) => {
      const i = Math.max(0, Math.min(index, queue.length - 1));
      if (!queue[i]) return;
      set({ inspector: { sessionId: queue[i], review: { queue, index: i } } });
    },
    reviewStep: (delta) =>
      set((s) => {
        const insp = s.inspector;
        if (!insp?.review) return {};
        const i = Math.max(0, Math.min(insp.review.index + delta, insp.review.queue.length - 1));
        return { inspector: { sessionId: insp.review.queue[i], review: { ...insp.review, index: i } } };
      }),
    closeInspector: () => set({ inspector: null, inspectorLive: false }),

    setReviewStatus: async (id, status) => {
      await setReviewStatusIpc(id, status);
      // Reflect it in the loaded list immediately (no full history reload/flicker).
      set((s) => ({
        sessions: s.sessions.map((sess) =>
          sess.id === id ? { ...sess, review_status: status } : sess,
        ),
      }));
    },

    setReviewStatusMany: async (ids, status) => {
      if (ids.length === 0) return;
      await setReviewStatusManyIpc(ids, status);
      const idSet = new Set(ids);
      set((s) => ({
        sessions: s.sessions.map((sess) =>
          idSet.has(sess.id) ? { ...sess, review_status: status } : sess,
        ),
      }));
    },

    setQuestion: (question) => set({ question }),

    ingestTurnEnded: (e) => {
      const id = e.session;
      // A turn this client drove settles through its own promise (driveTurn);
      // only a rejoined one has nobody else to close it out.
      if (!rejoined.delete(id)) return;
      set((s) => {
        let patch: Partial<AppState> = {};
        // Whatever it was parked on is moot once the turn is over.
        if (s.question?.session === id) patch = { ...patch, question: null };
        if (s.approvals[id]) {
          const approvals = { ...s.approvals };
          delete approvals[id];
          patch = { ...patch, approvals };
        }
        const thread = s.threads[id];
        if (thread !== undefined) {
          const items =
            e.error !== undefined
              ? appendRetryPrompt(thread, lastUserBubbleText(thread), [], e.error)
              : finalizeAssistant(thread, e.text ?? "");
          patch = { ...patch, threads: { ...s.threads, [id]: items } };
        }
        return patch;
      });
      settleRunStatus(id);
      get().refreshHistory();
      get().refreshTotalTokens();
      void refreshThreadsAfterSeen(id);
    },

    ingestApprovalRequest: (e) =>
      set((s) => ({ approvals: { ...s.approvals, [e.session]: e } })),

    ingestApprovalResolved: (e) =>
      set((s) => {
        if (e.phase !== "resolved") return {};
        const approvals = { ...s.approvals };
        delete approvals[e.session];
        const thread = s.threads[e.session];
        if (thread === undefined) return { approvals };
        return {
          approvals,
          threads: {
            ...s.threads,
            [e.session]: appendNotice(thread, `🛡 ${e.decision} — ${e.command}`),
          },
        };
      }),

    clearApproval: (session) =>
      set((s) => {
        const approvals = { ...s.approvals };
        delete approvals[session];
        return { approvals };
      }),
  };
});

/** The project the app is currently working in — the current chat's workspace,
 *  falling back to the backend's active project. Everything project-scoped
 *  (project skills, new chats) resolves against this. Returns `null` before a
 *  session exists. */
export function useActiveProject(): { path: string; name: string } | null {
  const session = useStore((s) => s.session);
  const projects = useStore((s) => s.projects);
  const path = session?.workspace ?? projects.find((p) => p.active)?.path ?? null;
  if (!path) return null;
  const name = projects.find((p) => p.path === path)?.name ?? path.split("/").pop() ?? path;
  return { path, name };
}

function initialMode(): Mode {
  const saved = getUi("mode") as Mode | undefined;
  const mode =
    saved ??
    (window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark");
  document.documentElement.dataset.theme = mode;
  return mode;
}
