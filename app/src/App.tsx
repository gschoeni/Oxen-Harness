import { useEffect, useState, type CSSProperties, type PointerEvent } from "react";
import { Sidebar } from "./features/history/Sidebar";
import { Chat } from "./features/chat/Chat";
import { Canvas } from "./features/canvas/Canvas";
import { Preview } from "./features/preview/Preview";
import { Settings } from "./features/settings/Settings";
import { ProjectsPage } from "./features/projects/ProjectsPage";
import { InspectorDrawer } from "./features/inspector/Inspector";
import { activeTheme } from "./lib/ipc";
import { useStore } from "./lib/store";
import "./app.css";

export default function App() {
  const applyTheme = useStore((s) => s.applyTheme);
  const loadSession = useStore((s) => s.loadSession);
  const refreshHistory = useStore((s) => s.refreshHistory);
  const refreshTotalTokens = useStore((s) => s.refreshTotalTokens);
  const loadCloudModels = useStore((s) => s.loadCloudModels);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const projectsOpen = useStore((s) => s.projectsOpen);
  // Show the canvas column when the current chat has an open document OR is
  // mid-write (so the panel appears the moment the model starts a canvas).
  const canvasOpen = useStore((s) => {
    const id = s.session?.session_id;
    if (!id) return false;
    if (s.canvasWriting[id]) return true;
    const active = s.activeCanvas[id];
    return !!active && !!s.canvases[id]?.some((d) => d.id === active);
  });
  // Show the preview column when the current chat has a dev server that is
  // starting, serving, or worth explaining (error/stopped — the pane holds the
  // Restart button, so it must not vanish the moment a server goes down).
  const previewOpen = useStore((s) => {
    const id = s.session?.session_id;
    if (!id || s.previewClosed[id]) return false;
    return !!s.previews[id];
  });
  // When both surfaces have content, the per-session tab decides which shows.
  const rightTab = useStore((s) => {
    const id = s.session?.session_id;
    return id ? s.rightTab[id] : undefined;
  });
  const sessionId = useStore((s) => s.session?.session_id);
  const syncPreview = useStore((s) => s.syncPreview);
  const showPreview = previewOpen && (!canvasOpen || rightTab !== "canvas");
  const showCanvas = canvasOpen && !showPreview;
  const panelOpen = showPreview || showCanvas;

  // A freshly opened/resumed chat may already have a running server (they
  // outlive agent eviction) — sync its status so the pane reappears.
  useEffect(() => {
    if (sessionId) syncPreview(sessionId).catch(() => {});
  }, [sessionId, syncPreview]);

  // Width (px) of the canvas column, drag-resizable and remembered across runs.
  const [canvasWidth, setCanvasWidth] = useState(() => {
    const saved = Number(localStorage.getItem(CANVAS_W_KEY));
    return saved >= CANVAS_MIN ? saved : 480;
  });
  const [resizing, setResizing] = useState(false);

  // Drag the divider: the canvas grows to the left as the cursor moves left,
  // clamped so the sidebar and a usable chat column always remain.
  function beginResize(e: PointerEvent) {
    e.preventDefault();
    setResizing(true);
    const move = (ev: globalThis.PointerEvent) => {
      const max = window.innerWidth - SIDEBAR_W - CHAT_MIN;
      const w = Math.max(CANVAS_MIN, Math.min(window.innerWidth - ev.clientX, max));
      setCanvasWidth(w);
    };
    const up = () => {
      setResizing(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      // Persist whatever width we ended on.
      setCanvasWidth((w) => {
        localStorage.setItem(CANVAS_W_KEY, String(w));
        return w;
      });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // Load the active theme, current session, and history once at startup.
  useEffect(() => {
    activeTheme().then(applyTheme).catch(() => {});
    loadSession().catch(() => {});
    refreshHistory();
    refreshTotalTokens();
    loadCloudModels();
  }, [applyTheme, loadSession, refreshHistory, refreshTotalTokens, loadCloudModels]);

  // Agent event subscriptions (tokens, tools, usage, canvas, questions) are set
  // up once in `startAgentEventBridge` (see main.tsx) — outside React's lifecycle
  // so StrictMode can't register duplicate listeners.

  return (
    <div
      className={`app${panelOpen ? " canvas-open" : ""}${resizing ? " resizing" : ""}`}
      style={panelOpen ? ({ "--canvas-w": `${canvasWidth}px` } as CSSProperties) : undefined}
    >
      <Sidebar />
      <Chat />
      {showPreview && <Preview onResizeStart={beginResize} />}
      {showCanvas && <Canvas onResizeStart={beginResize} />}
      {settingsOpen && <Settings />}
      {projectsOpen && <ProjectsPage />}
      <InspectorDrawer />
    </div>
  );
}

const CANVAS_W_KEY = "oxen-canvas-w";
const CANVAS_MIN = 320; // smallest the canvas column may shrink to
const CHAT_MIN = 380; // keep at least this much chat visible
const SIDEBAR_W = 272;
