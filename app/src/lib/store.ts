// Global app state. Chats are multi-session: each chat owns a thread, a run
// status, and a send queue keyed by session id, so a chat keeps streaming in the
// background after you switch to (or start) another. The store — not the Chat
// component — drives turns and routes streamed tokens, so progress survives
// unmounting the view. Also holds light/dark mode, the active theme, history,
// open overlays, and any pending clarifying question.

import { create } from "zustand";
import { lighten, readableOn, withAlpha } from "./color";
import {
  listProjects,
  listSessions,
  newSession,
  openProject,
  pickFolder,
  resumeSession,
  runTurn,
  sessionInfo,
  setActiveProject,
} from "./ipc";
import {
  appendToken,
  finalizeAssistant,
  startTurn,
  toolEnd,
  toolStart,
  transcriptToItems,
  type Item,
} from "../features/chat/thread";
import type {
  CanvasDoc,
  CanvasEvent,
  Mode,
  Project,
  QuestionPayload,
  RunStatus,
  SessionInfo,
  SessionSummary,
  Theme,
  ToolEvent,
} from "./types";

const MODE_KEY = "oxen-ui-mode";

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
  sessions: SessionSummary[];
  /** Known projects (working directories), refreshed alongside history. */
  projects: Project[];
  /** Whether the projects screen overlay is open. */
  projectsOpen: boolean;
  /** Known session infos by id, so switching to a live chat keeps its header. */
  infos: Record<string, SessionInfo>;
  /** Live thread items per session id. */
  threads: Record<string, Item[]>;
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
  settingsOpen: boolean;
  modelsOpen: boolean;
  themesOpen: boolean;
  question: QuestionPayload | null;

  setMode: (m: Mode) => void;
  toggleMode: () => void;
  applyTheme: (t: Theme) => void;
  refreshHistory: () => Promise<void>;
  loadSession: () => Promise<void>;
  startNewSession: () => Promise<void>;
  resume: (id: string) => Promise<void>;
  setProjectsOpen: (open: boolean) => void;
  /** Switch to a known project and start a fresh chat in it. */
  enterProject: (path: string) => Promise<void>;
  /** Pick a folder, register it as a project, and start a fresh chat in it. */
  createProject: () => Promise<void>;
  /** Adopt a fresh session created by a model/connection switch as the current
   *  chat (it starts empty on a new endpoint). */
  adoptSession: (info: SessionInfo) => void;
  /** Send (or queue) a prompt in the current chat. */
  send: (text: string, attachments?: string[]) => void;
  /** Replace the current chat's send queue (used by the queue editor). */
  setQueue: (items: string[]) => void;
  /** Route a streamed token / tool event into its session's thread. */
  ingestToken: (session: string, token: string) => void;
  ingestTool: (e: ToolEvent) => void;
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
  setModelsOpen: (open: boolean) => void;
  setThemesOpen: (open: boolean) => void;
  setQuestion: (q: QuestionPayload | null) => void;
}

export const useStore = create<AppState>((set, get) => {
  // Drive one turn for `id`, then either send the next queued prompt or settle
  // the run status (read if the chat is in view, unread if it finished offscreen).
  function runTurnFor(id: string, text: string, paths: string[]) {
    set((s) => ({
      threads: { ...s.threads, [id]: startTurn(s.threads[id] ?? [], text) },
      runStatus: { ...s.runStatus, [id]: "running" },
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
        // not leave the panel stuck in the writing state.
        set((s) => ({ canvasWriting: { ...s.canvasWriting, [id]: false } }));
        get().refreshHistory(); // the first turn gives a new session its title
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
    sessions: [],
    projects: [],
    projectsOpen: false,
    infos: {},
    threads: {},
    runStatus: {},
    queues: {},
    canvases: {},
    activeCanvas: {},
    canvasWriting: {},
    settingsOpen: false,
    modelsOpen: false,
    themesOpen: false,
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

    send: (text, attachments = []) => {
      const id = get().session?.session_id;
      if (!id) return;
      if (get().runStatus[id] === "running") {
        const prompt = { text, attachments };
        set((s) => ({ queues: { ...s.queues, [id]: [...(s.queues[id] ?? []), prompt] } }));
        return;
      }
      runTurnFor(id, text, attachments);
    },

    setQueue: (items) =>
      set((s) => {
        const id = s.session?.session_id;
        return id ? { queues: { ...s.queues, [id]: reconcileQueueTexts(s.queues[id], items) } } : {};
      }),

    ingestToken: (session, token) =>
      set((s) =>
        s.threads[session] === undefined
          ? {}
          : { threads: { ...s.threads, [session]: appendToken(s.threads[session], token) } },
      ),

    ingestTool: (e) =>
      set((s) => {
        if (s.threads[e.session] === undefined) return {};
        const updated =
          e.phase === "start"
            ? toolStart(s.threads[e.session], e.name, e.detail, Date.now())
            : toolEnd(s.threads[e.session], e.name, e.detail, Date.now());
        return { threads: { ...s.threads, [e.session]: updated } };
      }),

    ingestCanvas: ({ session, ...doc }) =>
      set((s) => {
        const list = s.canvases[session] ?? [];
        const i = list.findIndex((d) => d.id === doc.id);
        // Update in place if the id exists, else append. Opening it focuses the
        // panel on this doc for that session and clears the "writing" state.
        const next = i >= 0 ? list.map((d, j) => (j === i ? doc : d)) : [...list, doc];
        return {
          canvases: { ...s.canvases, [session]: next },
          activeCanvas: { ...s.activeCanvas, [session]: doc.id },
          canvasWriting: { ...s.canvasWriting, [session]: false },
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
        const next = i >= 0 ? list.map((d, j) => (j === i ? doc : d)) : [...list, doc];
        return {
          canvases: { ...s.canvases, [session]: next },
          activeCanvas: { ...s.activeCanvas, [session]: doc.id },
        };
      }),

    setCanvasWriting: (session, writing) =>
      set((s) => ({ canvasWriting: { ...s.canvasWriting, [session]: writing } })),

    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
    setModelsOpen: (modelsOpen) => set({ modelsOpen }),
    setThemesOpen: (themesOpen) => set({ themesOpen }),
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

/** Map the active harness theme's accent palette onto the app's accent tokens.
 *  Neutrals stay controlled by light/dark mode, so themes layer cleanly. */
export function applyThemePalette(theme: Theme) {
  const p = theme.palette;
  const root = document.documentElement.style;
  root.setProperty("--accent", p.primary);
  root.setProperty("--accent-hover", lighten(p.primary, 0.12));
  root.setProperty("--accent-soft", withAlpha(p.primary, 0.16));
  root.setProperty("--on-accent", readableOn(p.primary));
  root.setProperty("--focus", withAlpha(p.primary, 0.5));
  if (p.link) root.setProperty("--link", p.link);
  if (p.danger) root.setProperty("--danger", p.danger);
}

/** Map the active theme's `[style]` onto the typography + framing tokens, so the
 *  same components re-skin (8-bit trail, newspaper, soft Apple app, neon grid)
 *  from data alone. Missing fields keep the Oregon Trail defaults in pixel.css. */
export function applyThemeStyle(theme: Theme) {
  const st = theme.style;
  const root = document.documentElement.style;
  if (!st) return;

  root.setProperty("--font-display", st.font_display);
  root.setProperty("--font-body", st.font_body);
  root.setProperty("--font-readout", st.font_mono);
  root.setProperty("--frame-radius", st.radius);
  root.setProperty("--frame-border-w", st.border_width);
  root.setProperty("--label-transform", st.display_transform === "uppercase" ? "uppercase" : "none");
  root.setProperty("--label-spacing", st.display_spacing);
  root.setProperty("--wordmark-spacing", st.display_spacing);

  // A pixel hero needs the squat block face kept small; everything else gets a
  // larger, more conventional display size.
  const pixelHero = st.hero === "pixel";
  root.setProperty("--label-size", pixelHero ? "9px" : "11px");
  root.setProperty(
    "--wordmark-size",
    st.hero === "pixel"
      ? "clamp(15px, 4.4vw, 24px)"
      : st.hero === "newspaper"
        ? "clamp(30px, 8vw, 54px)"
        : "clamp(26px, 6.5vw, 40px)",
  );

  // Depth treatment → the three frame-shadow tokens + hover lift.
  const edge = "var(--pixel-edge)";
  let shadow = "none";
  let hover = "none";
  let lg = "none";
  let lift = "0px";
  switch (st.shadow) {
    case "pixel":
      shadow = `3px 3px 0 ${edge}`;
      hover = `4px 4px 0 ${edge}`;
      lg = `5px 5px 0 ${edge}`;
      lift = "-1px";
      break;
    case "soft":
      shadow = "0 1px 2px rgba(0,0,0,.10), 0 6px 20px rgba(0,0,0,.10)";
      hover = "0 2px 6px rgba(0,0,0,.12), 0 14px 36px rgba(0,0,0,.16)";
      lg = "0 16px 48px rgba(0,0,0,.18)";
      lift = "-1px";
      break;
    case "glow":
      shadow = "0 0 16px -4px var(--accent), 0 2px 0 rgba(0,0,0,.4)";
      hover = "0 0 24px -2px var(--accent), 0 2px 0 rgba(0,0,0,.4)";
      lg = "0 0 30px -4px var(--accent), 0 3px 0 rgba(0,0,0,.4)";
      lift = "-1px";
      break;
    case "none":
    default:
      break;
  }
  root.setProperty("--frame-shadow", shadow);
  root.setProperty("--frame-shadow-hover", hover);
  root.setProperty("--frame-shadow-lg", lg);
  root.setProperty("--lift", lift);

  // A coarse hook for hero-specific CSS (newspaper rules, minimal splash, …).
  document.documentElement.dataset.hero = st.hero;
}
