// Global app state. Chats are multi-session: each chat owns a thread, a run
// status, and a send queue keyed by session id, so a chat keeps streaming in the
// background after you switch to (or start) another. The store — not the Chat
// component — drives turns and routes streamed tokens, so progress survives
// unmounting the view. Also holds light/dark mode, the active theme, history,
// open overlays, and any pending clarifying question.

import { create } from "zustand";
import { applyThemePalette, applyThemeStyle } from "./theme";
import {
  deleteSession,
  listCloudModels,
  listProjects,
  listSessions,
  newSession,
  openProject,
  pickFolder,
  resumeSession,
  runTurn,
  cancelTurn,
  sessionInfo,
  setActiveProject,
  setModel,
  setReviewStatus as setReviewStatusIpc,
  setReviewStatusMany as setReviewStatusManyIpc,
  totalTokensUsed,
  useLocalModel,
} from "./ipc";
import {
  appendNotice,
  appendToken,
  finalizeAssistant,
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
  LocalStatus,
  Mode,
  Project,
  QuestionPayload,
  ReviewStatus,
  RunStatus,
  SessionInfo,
  SessionSummary,
  SettingsPage,
  Theme,
  ToolEvent,
  ToolDeltaEvent,
  UsageEvent,
  CompactedEvent,
} from "./types";

const MODE_KEY = "oxen-ui-mode";

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

/** Keep only the newest `MAX_CANVASES` docs. The just-touched doc is appended
 *  last (and is the active one), so it always survives the trim. */
function capCanvases(list: CanvasDoc[]): CanvasDoc[] {
  return list.length > MAX_CANVASES ? list.slice(list.length - MAX_CANVASES) : list;
}

export interface QueuedPrompt {
  text: string;
  attachments: string[];
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
  /** The chat currently shown. */
  session: SessionInfo | null;
  /** All-time total tokens used across every session (drives the hero's stat). */
  totalTokensUsed: number;
  sessions: SessionSummary[];
  /** Known projects (working directories), refreshed alongside history. */
  projects: Project[];
  /** The cloud model catalog (built-ins + custom), for the picker + settings. */
  cloudModels: CloudModel[];
  /** Whether the projects screen overlay is open. */
  projectsOpen: boolean;
  /** Known session infos by id, so switching to a live chat keeps its header. */
  infos: Record<string, SessionInfo>;
  /** Live thread items per session id. */
  threads: Record<string, Item[]>;
  /** Estimated tokens streamed in the current in-flight turn, per session — lets
   *  the usage meter tick up live before the authoritative count lands at turn
   *  end. Reset to 0 when that turn's `agent://usage` arrives. */
  liveTokens: Record<string, number>;
  /** Generation speed (tokens/sec) per session, measured over the current
   *  streaming burst. Persists the last rate when idle. */
  tokensPerSecond: Record<string, number>;
  /** Per-session run state driving the sidebar indicator (absent = idle/read). */
  runStatus: Record<string, RunStatus>;
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
  applyTheme: (t: Theme) => void;
  refreshHistory: () => Promise<void>;
  loadSession: () => Promise<void>;
  startNewSession: () => Promise<void>;
  resume: (id: string) => Promise<void>;
  /** Permanently delete a chat; if it was the current one, open a fresh chat. */
  removeSession: (id: string) => Promise<void>;
  setProjectsOpen: (open: boolean) => void;
  /** Switch to a known project and start a fresh chat in it. */
  enterProject: (path: string) => Promise<void>;
  /** Pick a folder, register it as a project, and start a fresh chat in it. */
  createProject: () => Promise<void>;
  /** Adopt a fresh session created by a model/connection switch as the current
   *  chat (it starts empty on a new endpoint). */
  adoptSession: (info: SessionInfo) => void;
  /** Refresh the cloud model catalog from the backend. */
  loadCloudModels: () => Promise<void>;
  /** Swap the current chat to a cloud model in place, continuing the same
   *  conversation (keeps the thread; only the model changes). */
  changeModel: (model: string) => Promise<void>;
  /** Switch to a downloaded local model — starts a fresh chat on it. */
  switchToLocalModel: (id: string) => Promise<void>;
  /** Live status while switching to a local model (its server is starting), so
   *  the UI can show progress instead of an opaque "Switching…". Null when idle. */
  localSwitch: { model: string; phase: LocalStatus["phase"]; startedAt: number } | null;
  /** Update the local-switch phase from a `local://status` event. */
  setLocalStatus: (s: LocalStatus) => void;
  /** Send (or queue) a prompt in the current chat. */
  send: (text: string, attachments?: string[]) => void;
  /** Stop the current chat's in-flight turn, killing the model stream. */
  stop: () => void;
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

  // Drive one turn for `id`, then either send the next queued prompt or settle
  // the run status (read if the chat is in view, unread if it finished offscreen).
  function runTurnFor(id: string, text: string, paths: string[]) {
    genSamples.delete(id); // a new turn starts a fresh speed measurement
    set((s) => ({
      threads: { ...s.threads, [id]: startTurn(s.threads[id] ?? [], text, paths) },
      runStatus: { ...s.runStatus, [id]: "running" },
      // Clear any stale live estimate so this turn's meter starts from the
      // authoritative base (the usage event normally resets it, but be safe).
      liveTokens: { ...s.liveTokens, [id]: 0 },
    }));
    runTurn(id, text, paths)
      .then((final) =>
        set((s) => ({ threads: { ...s.threads, [id]: finalizeAssistant(s.threads[id] ?? [], final) } })),
      )
      .catch((e) =>
        set((s) => ({
          threads: { ...s.threads, [id]: finalizeAssistant(s.threads[id] ?? [], `⚠ ${e}`, true) },
        })),
      )
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
        const next = (get().queues[id] ?? [])[0];
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

  return {
    mode: initialMode(),
    theme: null,
    session: null,
    totalTokensUsed: 0,
    sessions: [],
    projects: [],
    cloudModels: [],
    localSwitch: null,
    projectsOpen: false,
    infos: {},
    threads: {},
    liveTokens: {},
    tokensPerSecond: {},
    runStatus: {},
    queues: {},
    canvases: {},
    activeCanvas: {},
    canvasWriting: {},
    streamingTool: {},
    streamingCanvas: {},
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
        threads: { ...s.threads, [info.session_id]: s.threads[info.session_id] ?? [] },
      }));
    },

    // Start a fresh chat. Any running chat keeps going in the background.
    startNewSession: async () => {
      const info = await newSession();
      set((s) => ({
        session: info,
        infos: { ...s.infos, [info.session_id]: info },
        threads: { ...s.threads, [info.session_id]: [] },
      }));
      get().refreshHistory();
    },

    resume: async (id) => {
      if (id === get().session?.session_id) return;
      const view = await resumeSession(id);
      set((s) => {
        // A mid-turn chat (`running`) keeps its live in-memory thread + info; a
        // cold history session seeds its thread and info from the transcript.
        const threads =
          view.running || s.threads[id] !== undefined
            ? s.threads
            : { ...s.threads, [id]: transcriptToItems(view.messages) };
        const infos = view.running ? s.infos : { ...s.infos, [id]: view.info };
        const runStatus = { ...s.runStatus };
        if (runStatus[id] === "unread") delete runStatus[id]; // viewing it clears the dot
        return { session: infos[id] ?? view.info, threads, infos, runStatus };
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
          queues: drop(s.queues),
          canvases: drop(s.canvases),
          activeCanvas: drop(s.activeCanvas),
          canvasWriting: drop(s.canvasWriting),
          streamingTool: drop(s.streamingTool),
          streamingCanvas: drop(s.streamingCanvas),
          liveTokens: drop(s.liveTokens),
          tokensPerSecond: drop(s.tokensPerSecond),
        };
      });
      await get().refreshHistory();
      // If we deleted the chat in view, drop into a fresh one so the UI isn't empty.
      if (wasCurrent) await get().startNewSession();
    },

    setProjectsOpen: (projectsOpen) => set({ projectsOpen }),

    enterProject: async (path) => {
      await setActiveProject(path);
      set({ projectsOpen: false });
      // A fresh chat in the entered project; its existing chats stay in the
      // sidebar folder. startNewSession refreshes history + projects.
      await get().startNewSession();
    },

    createProject: async () => {
      const path = await pickFolder();
      if (!path) return;
      await openProject(path); // registers + makes active on the backend
      set({ projectsOpen: false });
      await get().startNewSession();
    },

    adoptSession: (info) =>
      set((s) => ({
        session: info,
        infos: { ...s.infos, [info.session_id]: info },
        threads: { ...s.threads, [info.session_id]: [] },
      })),

    loadCloudModels: async () => {
      try {
        set({ cloudModels: await listCloudModels() });
      } catch {
        /* leave the previous catalog in place on a transient error */
      }
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
      set((st) =>
        st.localSwitch
          ? { localSwitch: { ...st.localSwitch, model: s.model, phase: s.phase } }
          : {},
      ),

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
      void cancelTurn(id).catch(() => {});
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
        const args = prev && prev.name === e.name ? prev.args + e.delta : e.delta;
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
        };
      }),

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

    refreshTotalTokens: async () => {
      try {
        set({ totalTokensUsed: await totalTokensUsed() });
      } catch {
        /* leave the previous total in place on a transient error */
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
          // The committed doc supersedes both streaming buffers; release the raw
          // arg string too so a large doc isn't held twice once it lands.
          streamingCanvas: { ...s.streamingCanvas, [session]: undefined },
          streamingTool: { ...s.streamingTool, [session]: undefined },
        };
      }),

    setActiveCanvas: (id) =>
      set((s) => {
        const cur = s.session?.session_id;
        return cur ? { activeCanvas: { ...s.activeCanvas, [cur]: id } } : {};
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
        };
      }),

    setCanvasWriting: (session, writing) =>
      set((s) => ({ canvasWriting: { ...s.canvasWriting, [session]: writing } })),

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

function initialMode(): Mode {
  const saved = localStorage.getItem(MODE_KEY) as Mode | null;
  const mode =
    saved ??
    (window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark");
  document.documentElement.dataset.theme = mode;
  return mode;
}
