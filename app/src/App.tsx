import { useEffect, type CSSProperties } from "react";
import { Chat } from "./features/chat/Chat";
import { DockColumn, RAIL_W, useDockShortcuts } from "./features/docks/DockColumn";
import { docksOnSide, useAvailableDocks } from "./features/docks/docks";
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
  const leftWidth = useDockWidth("left");
  const rightWidth = useDockWidth("right");
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
          "--dock-left-w": leftWidth == null ? "0px" : `${leftWidth}px`,
          "--dock-right-w": rightWidth == null ? "0px" : `${rightWidth}px`,
        } as CSSProperties
      }
    >
      <DockColumn side="left" />
      <Chat />
      <DockColumn side="right" />
      {settingsOpen && <Settings />}
      {projectsOpen && <ProjectsPage />}
      <InspectorDrawer />
    </div>
  );
}

/** The grid width for a side: `null` when nothing is docked there (no column),
 *  the rail width when collapsed, else the (persisted, drag-set) width. */
function useDockWidth(side: "left" | "right"): number | null {
  const available = useAvailableDocks(side);
  const collapsed = useStore((s) => !!s.dockCollapsed[side]);
  const width = useStore((s) => s.dockWidths[side]);
  if (!available.length) return null;
  if (collapsed) return RAIL_W;
  // Fall back to the widest default among this side's docks, so a new dock
  // gets a sensible size before the user ever drags it.
  const fallback = Math.max(...docksOnSide(side).map((d) => d.defaultWidth));
  return width ?? fallback;
}
