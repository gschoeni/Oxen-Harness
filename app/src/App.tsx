import { useEffect, useState, type CSSProperties } from "react";
import { Bot } from "lucide-react";
import { TitleBar } from "./TitleBar";
import { Chat } from "./features/chat/Chat";
import { DockColumn, useActiveDock, useDockShortcuts } from "./features/docks/DockColumn";
import { docksOnSide, useAvailableDocks } from "./features/docks/docks";
import { planColumns, type ColumnPlan, type LayoutPlan } from "./features/docks/layout";
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
  const sessionId = useStore((s) => s.session?.session_id);
  const syncPreview = useStore((s) => s.syncPreview);

  // The layout is whatever the dock registry says: each side is a column of
  // however many docks currently have content (tabbed), independently sized
  // and collapsible. Adding a panel is a registry entry — see docks.tsx.
  // The solver keeps every column on screen when the window shrinks.
  const layout = useLayoutPlan();
  useDockShortcuts();

  // A freshly opened/resumed chat may already have a running server (they
  // outlive agent eviction) — sync its status so the pane reappears.
  useEffect(() => {
    if (sessionId) syncPreview(sessionId).catch(() => {});
  }, [sessionId, syncPreview]);

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
      className="app"
      style={
        {
          "--dock-left-w": columnPx(layout.left),
          "--dock-right-w": columnPx(layout.right),
        } as CSSProperties
      }
    >
      <TitleBar />
      <div className="app-columns">
        <DockColumn side="left" forceRail={!!layout.left?.railed} />
        {layout.chatRailed ? <ChatRail /> : <Chat />}
        <DockColumn side="right" forceRail={!!layout.right?.railed} />
      </div>
      {settingsOpen && <Settings />}
      {projectsOpen && <ProjectsPage />}
      <InspectorDrawer />
    </div>
  );
}

const columnPx = (column: ColumnPlan | null) => (column ? `${column.width}px` : "0px");

/** The fitted layout for the current window: each side's effective width and
 *  whether it (or, in the terminal squeeze, the chat itself) renders as a
 *  rail. Derived, never persisted — widening the window restores what the
 *  user had. */
function useLayoutPlan(): LayoutPlan {
  const windowWidth = useWindowWidth();
  const state = {
    left: useSideInput("left"),
    right: useSideInput("right"),
  };
  return planColumns(windowWidth, state.left, state.right);
}

function useSideInput(side: "left" | "right") {
  const available = useAvailableDocks(side).length > 0;
  const active = useActiveDock(side);
  const collapsed = useStore((s) => !!s.dockCollapsed[side]);
  const width = useStore((s) => s.dockWidths[side]);
  // Fall back to the widest default among this side's docks, so a new dock
  // gets a sensible size before the user ever drags it.
  const fallback = Math.max(...docksOnSide(side).map((d) => d.defaultWidth));
  return {
    available,
    collapsed,
    desired: width ?? fallback,
    min: active?.minWidth ?? 240,
  };
}

function useWindowWidth(): number {
  const [width, setWidth] = useState(window.innerWidth);
  useEffect(() => {
    const measure = () => setWidth(window.innerWidth);
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);
  return width;
}

/** The chat squeezed to its bar: one button that reclaims the space by
 *  folding both dock columns to their rails. The agent is never lost. */
function ChatRail() {
  const setDockCollapsed = useStore((s) => s.setDockCollapsed);
  return (
    <main className="chat chat-collapsed">
      <button
        className="dock-rail-btn"
        title="Show the agent"
        aria-label="Show the agent"
        onClick={() => {
          setDockCollapsed("left", true);
          setDockCollapsed("right", true);
        }}
      >
        <Bot size={18} />
      </button>
    </main>
  );
}
