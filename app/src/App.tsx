import { useEffect, useState, type CSSProperties, type PointerEvent } from "react";
import { Sidebar } from "./features/history/Sidebar";
import { Chat } from "./features/chat/Chat";
import { Canvas } from "./features/canvas/Canvas";
import { Settings } from "./features/settings/Settings";
import { ModelsModal } from "./features/models/ModelsModal";
import { ThemesModal } from "./features/themes/ThemesModal";
import { QuestionModal } from "./features/questions/QuestionModal";
import { activeTheme, onCanvas, onCanvasWriting, onQuestion, onToken, onTool } from "./lib/ipc";
import { useStore } from "./lib/store";
import "./app.css";

export default function App() {
  const applyTheme = useStore((s) => s.applyTheme);
  const loadSession = useStore((s) => s.loadSession);
  const refreshHistory = useStore((s) => s.refreshHistory);
  const setQuestion = useStore((s) => s.setQuestion);
  const ingestToken = useStore((s) => s.ingestToken);
  const ingestTool = useStore((s) => s.ingestTool);
  const ingestCanvas = useStore((s) => s.ingestCanvas);
  const setCanvasWriting = useStore((s) => s.setCanvasWriting);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const modelsOpen = useStore((s) => s.modelsOpen);
  const themesOpen = useStore((s) => s.themesOpen);
  const question = useStore((s) => s.question);
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
  }, [applyTheme, loadSession, refreshHistory]);

  // Route streamed tokens / tool activity into their session's thread at the app
  // level, so a background chat keeps updating even while its view is unmounted.
  useEffect(() => {
    const unToken = onToken((e) => ingestToken(e.session, e.token));
    const unTool = onTool(ingestTool);
    const unCanvas = onCanvas(ingestCanvas);
    const unCanvasWriting = onCanvasWriting((session) => setCanvasWriting(session, true));
    return () => {
      unToken.then((fn) => fn());
      unTool.then((fn) => fn());
      unCanvas.then((fn) => fn());
      unCanvasWriting.then((fn) => fn());
    };
  }, [ingestToken, ingestTool, ingestCanvas, setCanvasWriting]);

  // The agent's clarifying-question tool emits globally; surface it as a modal.
  useEffect(() => {
    const unlisten = onQuestion(setQuestion);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setQuestion]);

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
      {question && <QuestionModal />}
    </div>
  );
}

const CANVAS_W_KEY = "oxen-canvas-w";
const CANVAS_MIN = 320; // smallest the canvas column may shrink to
const CHAT_MIN = 380; // keep at least this much chat visible
const SIDEBAR_W = 272;
