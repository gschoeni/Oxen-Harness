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
  deleteSession,
  listCloudModels,
  listProjects,
  listSessions,
  newSession,
  resumeSession,
  runCodeReview as runCodeReviewIpc,
  runTurn,
  runLoop,
  retryTurn,
  cancelTurn,
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
  totalTokensUsed,
  useLocalModel,
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
  startTurn,
  toolEnd,
  toolStart,
  transcriptToItems,
  type Item,
} from "../features/chat/thread";
import { partialCanvasDoc } from "./streamingArgs";
import type {
  CanvasDoc,
  CanvasEvent,
  CloudModel,
  CodeReviewProgressEvent,
  FleetActivityEvent,
  FleetAgentEvent,
  FleetStartedEvent,
  LocalStatus,
  Mode,
  Project,
  QuestionPayload,
  ReviewStatus,
  RunStatus,
  SessionInfo,
  SessionSummary,
  SettingsPage,
  StartupModelChoice,
  Theme,
  ToolEvent,
  ToolDeltaEvent,
  UsageEvent,
  CompactedEvent,
  CompressionEvent,
  CompressionMode,
  PreviewConsoleEvent,
  PreviewEvent,
  PreviewStatus,
  RetryEvent,
} from "./types";

const MODE_KEY = "oxen-ui-mode";
const HERO_GAME_KEY = "oxen-hero-game";

/** One parallel subagent as shown in the chat's fleet panel. */
export interface FleetLane {
  name: string;
  status: "queued" | "running" | "done" | "failed";
  /** One-line rolling readout (tool name or the freshest streamed words). */
  activity: string;
  /** Rolling tail of everything the lane streamed, for the expanded view. */
  tail: string;
  tokens: number;
}

/** A running fleet: its lanes plus which one is expanded for watching. */
export interface FleetView {
  source: "review" | "turn";
  lanes: FleetLane[];
  focused: number | null;
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
const MAX_STREAMING_TOOL_ARGS = 256_000;

function capThreadSessions(
  threads: Record<string, Item[]>,
  runStatus: Record<string, RunStatus>,
  current: string,
): Record<string, Item[]> {
  const protectedIds = new Set([
    current,
    ...Object.entries(runStatus)
      .filter(([, status]) => status === "running")
      .map(([id]) => id),
  ]);
  const keep = new Set(protectedIds);
  for (const id of Object.keys(threads).reverse()) {
    if (keep.size >= MAX_CACHED_THREADS && !protectedIds.has(id)) continue;
    keep.add(id);
  }
  return Object.fromEntries(Object.entries(threads).filter(([id]) => keep.has(id)));
}

function retainCached<T>(record: Record<string, T>, threads: Record<string, Item[]>): Record<string, T> {
  const ids = new Set(Object.keys(threads));
  return Object.fromEntries(Object.entries(record).filter(([id]) => ids.has(id)));
}

/** Keep only the newest `MAX_CANVASES` docs. The just-touched doc is appended
 *  last (and is the active one), so it always survives the trim. */
function capCanvases(list: CanvasDoc[]): CanvasDoc[] {
  return list.length > MAX_CANVASES ? list.slice(list.length - MAX_CANVASES) : list;
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
  mode: Mode;
  theme: Theme | null;
  /** Which empty-state hero game the player has chosen (persisted). Null falls
   *  back to the active theme's default game. Shared by the hero and the
   *  play-while-you-work game dock so both show the same cabinet. */
  heroGame: string | null;
  /** Whether the floating game dock is open (lets you play during a live turn). */
  gameDockOpen: boolean;
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
  /** Whether the projects screen overlay is open. */
  projectsOpen: boolean;
  /** Project whose getting-started/files page was opened explicitly. Null shows
   *  the project list instead. */
  projectHomePath: string | null;
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
  fleets: Record<string, FleetView | undefined>;
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
  /** Which right-panel tab is active per session when both preview and canvas
   *  have content. */
  rightTab: Record<string, "preview" | "canvas">;
  settingsOpen: boolean;
  /** Which subpage the full-screen Settings surface shows when open. */
  settingsPage: SettingsPage;
  /** The transcript inspector drawer. `review` is set when opened from the
   *  dataset builder (carries the queue of chats to page through); null =
   *  plain inspection (e.g. the chat's </> button). Absent = drawer closed. */
  inspector: { sessionId: string; review: { queue: string[]; index: number } | null } | null;
  question: QuestionPayload | null;

  setMode: (m: Mode) => void;
  toggleMode: () => void;
  /** Choose (and persist) the empty-state hero game. */
  setHeroGame: (name: string) => void;
  /** Open/close the floating game dock. */
  setGameDockOpen: (open: boolean) => void;
  applyTheme: (t: Theme) => void;
  refreshHistory: () => Promise<void>;
  loadSession: () => Promise<void>;
  startNewSession: () => Promise<void>;
  resume: (id: string) => Promise<void>;
  /** Permanently delete a chat; if it was the current one, open a fresh chat. */
  removeSession: (id: string) => Promise<void>;
  setProjectsOpen: (open: boolean) => void;
  /** Open a project's getting-started, guidance, and reference-files page. */
  openProjectHome: (path: string) => void;
  /** Make project-scoped surfaces point at a project without creating a chat. */
  selectProject: (path: string) => Promise<void>;
  /** Prepare a known project and fresh chat while keeping its home visible. */
  prepareProject: (path: string, model?: StartupModelChoice) => Promise<void>;
  /** Switch to a known project and enter its fresh chat. */
  enterProject: (path: string) => Promise<void>;
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
  /** Send (or queue) a prompt in the current chat. */
  send: (text: string, attachments?: string[]) => void;
  /** Run the code-review pipeline in the current chat's workspace (uncommitted
   *  changes, or PR-style against `baseBranch`). The findings land in the thread
   *  as a settled exchange, so a follow-up "fix 1 and 3" just works. */
  startCodeReview: (baseBranch?: string) => void;
  /** Run a saved loop, or an ad-hoc goal, in the current chat. */
  startLoop: (name?: string, goal?: string) => void;
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
  /** The fleet finished: close the session's panel. */
  ingestFleetCompleted: (session: string) => void;
  /** Expand one lane to watch its output (null collapses back to the list). */
  setFleetFocus: (session: string, index: number | null) => void;
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
  ingestTool: (e: ToolEvent) => void;
  /** Accumulate a streaming tool-args fragment (live file/canvas preview). */
  ingestToolDelta: (e: ToolDeltaEvent) => void;
  /** Update a session's live usage (per-session count + context fill) as it
   *  accrues within a turn. */
  ingestUsage: (e: UsageEvent) => void;
  /** Add a notice to a session's thread when its context was compacted. */
  ingestCompacted: (e: CompactedEvent) => void;
  /** Add a notice when a model call hit a transient error and is retrying. */
  ingestRetry: (e: RetryEvent) => void;
  /** Update a session's compression savings counters. Fires per model call —
   *  deliberately no thread notice (that would be far too chatty). */
  ingestCompression: (e: CompressionEvent) => void;
  /** Refresh the all-time total tokens used from the backend. */
  refreshTotalTokens: () => Promise<void>;
  /** Upsert a canvas document and open it in the side panel. */
  ingestCanvas: (e: CanvasEvent) => void;
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
  /** Switch the current chat's right-panel tab (preview ⇄ canvas). */
  setRightTab: (tab: "preview" | "canvas") => void;
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
}

export const useStore = create<AppState>((set, get) => {
  // Non-reactive per-session sample for the tokens/sec readout: the start of the
  // current streaming burst and tokens seen in it. A burst resets after a gap
  // (tool calls), so the rate reflects active decoding, not idle time.
  const genSamples = new Map<string, { start: number; tokens: number; last: number }>();
  const BURST_GAP_MS = 1200;

  // Drive one turn for `id` (a fresh send, or a retry that continues the existing
  // transcript), then either send the next queued prompt or settle the run status
  // (read if the chat is in view, unread if it finished offscreen). The turn's UI
  // (user bubble + streaming assistant bubble) must already be in the thread.
  function driveTurn(id: string, text: string, paths: string[], retry: boolean) {
    genSamples.delete(id); // each turn starts a fresh speed measurement
    set((s) => ({
      runStatus: { ...s.runStatus, [id]: "running" },
      // Clear any stale live estimate so this turn's meter starts from the
      // authoritative base (the usage event normally resets it, but be safe).
      liveTokens: { ...s.liveTokens, [id]: 0 },
    }));
    // A retry continues the failed turn's transcript in place; a fresh turn sends
    // the prompt (and any attachments) for the first time.
    let recovering = false;
    const turn = retry ? retryTurn(id) : runTurn(id, text, paths);
    turn
      .then((final) =>
        set((s) => ({ threads: { ...s.threads, [id]: finalizeAssistant(s.threads[id] ?? [], final) } })),
      )
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
        set((s) => {
          const thread = s.threads[id] ?? [];
          return {
            threads: {
              ...s.threads,
              [id]: auth
                ? appendApiKeyPrompt(thread, text, paths)
                : appendRetryPrompt(thread, text, paths, message),
            },
          };
        });
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
        if (next !== undefined) {
          set((s) => ({ queues: { ...s.queues, [id]: (s.queues[id] ?? []).slice(1) } }));
          setTimeout(() => runTurnFor(id, next.text, next.attachments), 0); // let state settle first
        } else {
          set((s) => {
            const runStatus = { ...s.runStatus };
            if (s.session?.session_id === id) delete runStatus[id]; // in view → read
            else runStatus[id] = "unread";
            return { runStatus };
          });
        }
      });
  }

  // Start a fresh turn: append its UI (user bubble + empty streaming reply), then
  // drive it. A pending retry card is dropped — the new prompt supersedes it (its
  // dangling user turn is still in the transcript for the model to answer).
  function runTurnFor(id: string, text: string, paths: string[]) {
    set((s) => ({
      threads: { ...s.threads, [id]: startTurn(dropRetryPrompts(s.threads[id] ?? []), text, paths) },
    }));
    driveTurn(id, text, paths, false);
  }

  return {
    mode: initialMode(),
    theme: null,
    heroGame: localStorage.getItem(HERO_GAME_KEY),
    gameDockOpen: false,
    session: null,
    totalTokensUsed: 0,
    totalCostUsd: null,
    sessions: [],
    projects: [],
    cloudModels: [],
    localSwitch: null,
    // Projects is the application's navigation root. Established projects
    // enter their latest chat; their home is opened explicitly from chat.
    projectsOpen: true,
    projectHomePath: null,
    infos: {},
    threads: {},
    liveTokens: {},
    sessionUsage: {},
    tokensPerSecond: {},
    compression: {},
    runStatus: {},
    codeReview: {},
    fleets: {},
    queues: {},
    canvases: {},
    activeCanvas: {},
    canvasWriting: {},
    streamingTool: {},
    streamingCanvas: {},
    previews: {},
    previewClosed: {},
    previewErrors: {},
    rightTab: {},
    settingsOpen: false,
    settingsPage: "connection",
    inspector: null,
    question: null,

    setMode: (mode) => {
      document.documentElement.dataset.theme = mode;
      localStorage.setItem(MODE_KEY, mode);
      set({ mode });
    },
    toggleMode: () => get().setMode(get().mode === "light" ? "dark" : "light"),

    setHeroGame: (name) => {
      localStorage.setItem(HERO_GAME_KEY, name);
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
      } catch {
        /* leave the previous lists in place on a transient error */
      }
    },

    loadSession: async () => {
      const info = await sessionInfo();
      set((s) => ({
        session: info,
        infos: { ...s.infos, [info.session_id]: info },
        threads: capThreadSessions(
          { ...s.threads, [info.session_id]: s.threads[info.session_id] ?? [] },
          s.runStatus,
          info.session_id,
        ),
      }));
    },

    // Start a fresh chat. Any running chat keeps going in the background.
    startNewSession: async () => {
      const info = await newSession();
      set((s) => {
        const threads = capThreadSessions({ ...s.threads, [info.session_id]: [] }, s.runStatus, info.session_id);
        return {
          session: info,
          infos: { ...retainCached(s.infos, threads), [info.session_id]: info },
          threads,
          canvases: retainCached(s.canvases, threads),
          activeCanvas: retainCached(s.activeCanvas, threads),
          codeReview: retainCached(s.codeReview, threads),
          fleets: retainCached(s.fleets, threads),
          queues: retainCached(s.queues, threads),
          canvasWriting: retainCached(s.canvasWriting, threads),
          streamingTool: retainCached(s.streamingTool, threads),
          streamingCanvas: retainCached(s.streamingCanvas, threads),
          liveTokens: retainCached(s.liveTokens, threads),
          sessionUsage: retainCached(s.sessionUsage, threads),
          tokensPerSecond: retainCached(s.tokensPerSecond, threads),
          compression: retainCached(s.compression, threads),
        };
      });
      get().refreshHistory();
    },

    resume: async (id) => {
      if (id === get().session?.session_id) return;
      const view = await resumeSession(id);
      set((s) => {
        // A mid-turn chat (`running`) keeps its live in-memory thread + info; a
        // cold history session seeds its thread and info from the transcript.
        // A cold transcript that stops mid-turn (the reply never arrived — an
        // error, out of credits, or the app closed) gets an inline retry card
        // so the chat can be continued with one click.
        let seeded: Item[] | undefined;
        if (!view.running && s.threads[id] === undefined) {
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
        const threads = capThreadSessions(
          seeded === undefined ? s.threads : { ...s.threads, [id]: seeded },
          s.runStatus,
          id,
        );
        const infos = view.running ? s.infos : { ...s.infos, [id]: view.info };
        const runStatus = { ...s.runStatus };
        if (runStatus[id] === "unread") delete runStatus[id]; // viewing it clears the dot
        return {
          session: infos[id] ?? view.info,
          threads,
          infos: retainCached(infos, threads),
          runStatus,
          canvases: retainCached(s.canvases, threads),
          activeCanvas: retainCached(s.activeCanvas, threads),
          codeReview: retainCached(s.codeReview, threads),
          fleets: retainCached(s.fleets, threads),
          queues: retainCached(s.queues, threads),
          canvasWriting: retainCached(s.canvasWriting, threads),
          streamingTool: retainCached(s.streamingTool, threads),
          streamingCanvas: retainCached(s.streamingCanvas, threads),
          liveTokens: retainCached(s.liveTokens, threads),
          sessionUsage: retainCached(s.sessionUsage, threads),
          tokensPerSecond: retainCached(s.tokensPerSecond, threads),
          compression: retainCached(s.compression, threads),
        };
      });
      get().refreshHistory();
    },

    removeSession: async (id) => {
      await deleteSession(id);
      const wasCurrent = get().session?.session_id === id;
      genSamples.delete(id);
      // Forget every per-session slice so nothing lingers for the deleted chat.
      set((s) => {
        const drop = <T,>(rec: Record<string, T>) => {
          const copy = { ...rec };
          delete copy[id];
          return copy;
        };
        return {
          session: wasCurrent ? null : s.session,
          threads: drop(s.threads),
          infos: drop(s.infos),
          runStatus: drop(s.runStatus),
          codeReview: drop(s.codeReview),
          fleets: drop(s.fleets),
          queues: drop(s.queues),
          canvases: drop(s.canvases),
          activeCanvas: drop(s.activeCanvas),
          canvasWriting: drop(s.canvasWriting),
          streamingTool: drop(s.streamingTool),
          streamingCanvas: drop(s.streamingCanvas),
          liveTokens: drop(s.liveTokens),
          sessionUsage: drop(s.sessionUsage),
          tokensPerSecond: drop(s.tokensPerSecond),
          compression: drop(s.compression),
          // The backend stops the deleted chat's dev server and closes its
          // webview; drop the mirrored state so no stopped server lingers in
          // the sidebar chips or the Settings → Preview list.
          previews: drop(s.previews),
          previewClosed: drop(s.previewClosed),
          previewErrors: drop(s.previewErrors),
          rightTab: drop(s.rightTab),
        };
      });
      await get().refreshHistory();
      // If we deleted the chat in view, drop into a fresh one so the UI isn't empty.
      if (wasCurrent) await get().startNewSession();
    },

    setProjectsOpen: (projectsOpen) => set({ projectsOpen, projectHomePath: null }),

    openProjectHome: (projectHomePath) => set({ projectsOpen: true, projectHomePath }),

    selectProject: async (path) => {
      await setActiveProject(path);
      await get().refreshHistory();
    },

    prepareProject: async (path, model) => {
      await setActiveProject(path);
      if (model?.local) {
        await get().switchToLocalModel(model.id);
      } else {
        if (model) await selectCloudModelForNewChats(model.id);
        await get().startNewSession();
      }
    },

    enterProject: async (path) => {
      await get().prepareProject(path);
      set({ projectsOpen: false });
    },

    adoptSession: (info) =>
      set((s) => {
        const threads = capThreadSessions({ ...s.threads, [info.session_id]: [] }, s.runStatus, info.session_id);
        return {
          session: info,
          infos: { ...retainCached(s.infos, threads), [info.session_id]: info },
          threads,
          canvases: retainCached(s.canvases, threads),
          activeCanvas: retainCached(s.activeCanvas, threads),
          codeReview: retainCached(s.codeReview, threads),
          fleets: retainCached(s.fleets, threads),
          queues: retainCached(s.queues, threads),
          canvasWriting: retainCached(s.canvasWriting, threads),
          streamingTool: retainCached(s.streamingTool, threads),
          streamingCanvas: retainCached(s.streamingCanvas, threads),
          liveTokens: retainCached(s.liveTokens, threads),
          sessionUsage: retainCached(s.sessionUsage, threads),
          tokensPerSecond: retainCached(s.tokensPerSecond, threads),
          compression: retainCached(s.compression, threads),
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

    setLocalStatus: (s) =>
      set((st) => {
        // "ready"/"error" means the load is over — clear the inline state.
        if (s.phase === "ready" || s.phase === "error")
          return st.localSwitch ? { localSwitch: null } : {};
        // Create-or-update: a load that wasn't user-initiated (a persisted
        // local model starting lazily on the first call after an app relaunch)
        // must surface the same way an explicit switch does.
        return {
          localSwitch: st.localSwitch
            ? { ...st.localSwitch, model: s.model, phase: s.phase }
            : { model: s.model, phase: s.phase, startedAt: Date.now() },
        };
      }),

    send: (text, attachments = []) => {
      const id = get().session?.session_id;
      if (!id) return;
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
            const fleets = { ...s.fleets };
            delete fleets[id];
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
            set((s) => {
              const runStatus = { ...s.runStatus };
              if (s.session?.session_id === id) delete runStatus[id];
              else runStatus[id] = "unread";
              return { runStatus };
            });
          }
        });
    },

    startLoop: (name, goal) => {
      const id = get().session?.session_id;
      if (!id || get().runStatus[id] === "running") return;
      const label = goal ? `/loop goal ${goal}` : `/loop run ${name ?? "default"}`;
      set((s) => ({
        runStatus: { ...s.runStatus, [id]: "running" },
        threads: { ...s.threads, [id]: startTurn(s.threads[id] ?? [], label, []) },
      }));
      runLoop(id, name, goal)
        .then((result) =>
          set((s) => ({
            threads: {
              ...s.threads,
              [id]: finalizeAssistant(s.threads[id] ?? [], result.summary),
            },
          })),
        )
        .catch((error) =>
          set((s) => ({
            threads: {
              ...s.threads,
              [id]: finalizeAssistant(s.threads[id] ?? [], `Loop failed: ${String(error)}`),
            },
          })),
        )
        .finally(() => {
          set((s) => {
            const runStatus = { ...s.runStatus };
            delete runStatus[id];
            return { runStatus };
          });
          get().refreshHistory();
          get().refreshTotalTokens();
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
          [e.session]: {
            source: e.source,
            focused: null,
            lanes: e.agents.map((name) => ({
              name,
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
        const fleet = s.fleets[e.session];
        const lane = fleet?.lanes[e.agent];
        if (!fleet || !lane) return {};
        const updated: FleetLane =
          e.phase === "started"
            ? { ...lane, status: "running" }
            : {
                ...lane,
                status: e.phase,
                tokens: e.tokens,
                activity: e.summary || lane.activity,
              };
        const lanes = fleet.lanes.map((l, i) => (i === e.agent ? updated : l));
        return { fleets: { ...s.fleets, [e.session]: { ...fleet, lanes } } };
      }),

    ingestFleetActivity: (e) =>
      set((s) => {
        const fleet = s.fleets[e.session];
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
        } else {
          updated = { ...lane, tokens: e.tokens ?? lane.tokens };
        }
        const lanes = fleet.lanes.map((l, i) => (i === e.agent ? updated : l));
        return { fleets: { ...s.fleets, [e.session]: { ...fleet, lanes } } };
      }),

    ingestFleetCompleted: (session) =>
      set((s) => {
        if (!s.fleets[session]) return {};
        const fleets = { ...s.fleets };
        delete fleets[session];
        return { fleets };
      }),

    setFleetFocus: (session, index) =>
      set((s) => {
        const fleet = s.fleets[session];
        if (!fleet) return {};
        const focused = index !== null && index < fleet.lanes.length ? index : null;
        return { fleets: { ...s.fleets, [session]: { ...fleet, focused } } };
      }),

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
      // Retire the card, open a fresh reply bubble, and retry the failed turn
      // (which continues the existing transcript — no duplicate user message).
      set((s) => ({
        threads: { ...s.threads, [session]: resolveRecoveryPrompt(s.threads[session] ?? [], itemId) },
      }));
      driveTurn(session, item.text, item.attachments, true);
    },

    retryBrokenTurn: (session, itemId) => {
      if (get().runStatus[session] === "running") return; // a turn is already in flight
      const item = (get().threads[session] ?? []).find((it) => it.id === itemId);
      if (!item || item.kind !== "retry") return;
      set((s) => ({
        threads: { ...s.threads, [session]: resolveRecoveryPrompt(s.threads[session] ?? [], itemId) },
      }));
      driveTurn(session, item.text, item.attachments, true);
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
      set((s) => ({
        threads: { ...s.threads, [session]: appendToken(s.threads[session], token) },
        // Tick the usage meter up live as the reply streams, matching the
        // backend's ~4-chars-per-token estimate; snapped exact at turn end.
        liveTokens: { ...s.liveTokens, [session]: (s.liveTokens[session] ?? 0) + est },
        ...(tps !== null
          ? { tokensPerSecond: { ...s.tokensPerSecond, [session]: tps } }
          : {}),
      }));
    },

    ingestTool: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        const updated =
          e.phase === "start"
            ? toolStart(s.threads[e.session], e.name, e.detail, Date.now())
            : toolEnd(s.threads[e.session], e.name, e.detail, Date.now());
        // The call's args are fully assembled now (the real tool chip takes
        // over), so drop the streaming file preview. Canvas keeps its provisional
        // doc until the committed version lands via ingestCanvas.
        return {
          threads: { ...s.threads, [e.session]: updated },
          streamingTool: { ...s.streamingTool, [e.session]: undefined },
        };
      }),

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

    ingestUsage: (e) =>
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
      }),

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

    ingestRetry: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        const wait = Math.max(1, Math.ceil(e.delay_ms / 1000));
        return {
          threads: {
            ...s.threads,
            [e.session]: appendNotice(
              s.threads[e.session],
              `Model call failed (${e.error}) — retrying in ${wait}s (attempt ${e.attempt + 1} of ${e.max_attempts})`,
            ),
          },
        };
      }),

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

    setRightTab: (tab) =>
      set((s) => {
        const cur = s.session?.session_id;
        if (!cur) return {};
        return {
          rightTab: { ...s.rightTab, [cur]: tab },
          // Choosing the Preview tab means "show me the preview" — a pane the
          // user closed earlier must reopen, or the tab would be a dead click.
          ...(tab === "preview" ? { previewClosed: { ...s.previewClosed, [cur]: false } } : {}),
        };
      }),

    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
    openSettings: (page) =>
      set(page ? { settingsOpen: true, settingsPage: page } : { settingsOpen: true }),
    setSettingsPage: (settingsPage) => set({ settingsPage }),

    openInspector: (sessionId) => set({ inspector: { sessionId, review: null } }),
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
    closeInspector: () => set({ inspector: null }),

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
  const saved = localStorage.getItem(MODE_KEY) as Mode | null;
  const mode =
    saved ??
    (window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark");
  document.documentElement.dataset.theme = mode;
  return mode;
}
