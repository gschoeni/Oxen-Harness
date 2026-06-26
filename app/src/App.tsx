import { useEffect, useState, type CSSProperties, type PointerEvent } from "react";
import { Sidebar } from "./features/history/Sidebar";
import { Chat } from "./features/chat/Chat";
import { Canvas } from "./features/canvas/Canvas";
import { Settings } from "./features/settings/Settings";
import { ModelsModal } from "./features/models/ModelsModal";
import { ThemesModal } from "./features/themes/ThemesModal";
import { ProjectsModal } from "./features/projects/ProjectsModal";
import { DevView } from "./features/dev/DevView";
import { activeTheme } from "./lib/ipc";
import { useStore } from "./lib/store";
import "./app.css";

export default function App() {
  const applyTheme = useStore((s) => s.applyTheme);
  const loadSession = useStore((s) => s.loadSession);
  const refreshHistory = useStore((s) => s.refreshHistory);
  const refreshTotalTokens = useStore((s) => s.refreshTotalTokens);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const modelsOpen = useStore((s) => s.modelsOpen);
  const themesOpen = useStore((s) => s.themesOpen);
  const devViewOpen = useStore((s) => s.devViewOpen);
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
  }, [applyTheme, loadSession, refreshHistory, refreshTotalTokens]);

  // Agent event subscriptions (tokens, tools, usage, canvas, questions) are set
  // up once in `startAgentEventBridge` (see main.tsx) — outside React's lifecycle
  // so StrictMode can't register duplicate listeners.

  return (
    <div
      className={`app${canvasOpen ? " canvas-open" : ""}${resizing ? " resizing" : ""}`}
      style={canvasOpen ? ({ "--canvas-w": `${canvasWidth}px` } as CSSProperties) : undefined}
    >
      <Sidebar />
      <Chat />
      {canvasOpen && <Canvas onResizeStart={beginResize} />}
      {settingsOpen && <Settings />}
      {modelsOpen && <ModelsModal />}
      {themesOpen && <ThemesModal />}
      {devViewOpen && <DevView />}
      {projectsOpen && <ProjectsModal />}
    </div>
  );
}

const CANVAS_W_KEY = "oxen-canvas-w";
const CANVAS_MIN = 320; // smallest the canvas column may shrink to
const CHAT_MIN = 380; // keep at least this much chat visible
const SIDEBAR_W = 272;
