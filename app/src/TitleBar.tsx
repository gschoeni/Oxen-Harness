// The application title bar: one persistent strip across the very top of the
// window, above every column and overlay. It is the window's drag handle and
// double-clicks to zoom — exactly like a native title bar — so no pane below
// ever has to fake its own. The active project names the window; the chat's
// utility buttons (project home, arcade, inspector) sit at its right edge,
// clear of the macOS traffic lights on the left.

import { Activity, Code2, Files, Gamepad2 } from "lucide-react";
import { setUi } from "./lib/uiState";
import { useActiveProject, useStore } from "./lib/store";

export function TitleBar() {
  const sessionId = useStore((s) => s.session?.session_id);
  const hasThread = useStore(
    (s) => !!s.session && (s.threads[s.session.session_id]?.length ?? 0) > 0,
  );
  const projectPath = useStore((s) => {
    const path = s.session?.workspace;
    return path && s.projects.some((project) => project.path === path) ? path : null;
  });
  const project = useActiveProject();
  const openProjectHome = useStore((s) => s.openProjectHome);
  const gameDockOpen = useStore((s) => s.gameDockOpen);
  const setGameDockOpen = useStore((s) => s.setGameDockOpen);
  const openInspector = useStore((s) => s.openInspector);
  const runningCount = useStore((s) => {
    const sessions = new Set(s.ledger?.running ?? []);
    for (const [id, status] of Object.entries(s.runStatus)) {
      // Live state wins over an older Ledger snapshot in either direction.
      if (status === "running") sessions.add(id);
      else sessions.delete(id);
    }
    for (const [id, fleet] of Object.entries(s.fleets)) {
      if (fleet?.lanes.some((lane) => lane.status === "queued" || lane.status === "running")) {
        sessions.add(id);
      }
    }
    let count = 0;
    for (const id of sessions) {
      const activeLanes = s.fleets[id]?.lanes.filter(
        (lane) => lane.status === "queued" || lane.status === "running",
      ).length;
      count += activeLanes || 1;
    }
    return count;
  });
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);

  function openLedger() {
    setUi("homeView", "ledger");
    setSettingsOpen(false);
    setHomeOpen(true);
  }

  return (
    <header className="app-titlebar" data-tauri-drag-region>
      {/* pointer-events: none, so the name never blocks the drag region */}
      <span className="app-titlebar-name">{project?.name ?? "oxen-harness"}</span>
      <div className="app-titlebar-actions">
        <button
          className={`titlebar-running ${runningCount > 0 ? "active" : ""}`}
          onClick={openLedger}
          title={`${runningCount} running — open the Ledger`}
          aria-label={`${runningCount} running — open the Ledger`}
        >
          <Activity size={14} />
          <span>{runningCount}</span>
          <span className="titlebar-running-label">running</span>
        </button>
        {projectPath && (
          <button
            className="dev-view-btn"
            onClick={() => openProjectHome(projectPath)}
            title="Open this project's getting started, instructions, and files"
            aria-label="Project files and settings"
          >
            <Files size={15} />
          </button>
        )}
        {hasThread && (
          <button
            className="dev-view-btn"
            onClick={() => setGameDockOpen(!gameDockOpen)}
            aria-pressed={gameDockOpen}
            title="Play a game while your agent works"
            aria-label="Toggle the arcade"
          >
            <Gamepad2 size={15} />
          </button>
        )}
        <button
          className="dev-view-btn"
          onClick={() => sessionId && openInspector(sessionId)}
          disabled={!sessionId}
          title="Inspect this chat — the raw LLM inputs and outputs for this session"
          aria-label="Inspect this chat's transcript"
        >
          <Code2 size={15} />
        </button>
      </div>
    </header>
  );
}
