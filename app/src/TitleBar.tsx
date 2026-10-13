// The application title bar: one persistent strip across the very top of the
// window, above every column and overlay. It is the window's drag handle and
// double-clicks to zoom — exactly like a native title bar — so no pane below
// ever has to fake its own. The active project names the window; the running
// count (with machine memory beside it while a local model runs) and, inside
// a project, the panel switch (Files | Agent | Work | </>) sit at its right
// edge, clear of the macOS traffic lights. Settings is ⌘, (or Home); the
// study game is ⌘J (or the composer's button while a turn runs).

import { Activity } from "lucide-react";
import { useMemoryPolling } from "./lib/memory";
import { useActiveProject, useStore } from "./lib/store";
import { MemoryMeter } from "./features/chat/MemoryMeter";
import { PanelSwitch } from "./features/docks/PanelSwitch";

export function TitleBar() {
  // Polled here, not by the chat: the readout stays live on Home and in
  // Settings, where a local model can still be squeezing the machine.
  useMemoryPolling();
  const project = useActiveProject();
  const runningCount = useStore((s) => {
    const sessions = new Set(s.threadsSnapshot?.running ?? []);
    for (const [id, status] of Object.entries(s.runStatus)) {
      // Live state wins over an older snapshot in either direction.
      if (status === "running") sessions.add(id);
      else sessions.delete(id);
    }
    // Fleets are keyed by fleet id and a chat can run more than one at once:
    // its active lanes add up across them.
    const activeLanes = new Map<string, number>();
    for (const fleet of Object.values(s.fleets)) {
      if (!fleet) continue;
      const active = fleet.lanes.filter(
        (lane) => lane.status === "queued" || lane.status === "running",
      ).length;
      if (active > 0) {
        sessions.add(fleet.session);
        activeLanes.set(fleet.session, (activeLanes.get(fleet.session) ?? 0) + active);
      }
    }
    let count = 0;
    for (const id of sessions) {
      count += activeLanes.get(id) || 1;
    }
    return count;
  });
  // The panels only exist inside a project's chat view: over Home or
  // Settings, or with no project open, the switch would control nothing.
  const inProject = useStore((s) => !!s.session?.workspace && !s.homeOpen && !s.settingsOpen);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);

  function openHome() {
    setSettingsOpen(false);
    setHomeOpen(true);
  }

  return (
    <header className="app-titlebar" data-tauri-drag-region>
      {/* pointer-events: none, so the name never blocks the drag region */}
      <span className="app-titlebar-name">{project?.name ?? "oxen-harness"}</span>
      <div className="app-titlebar-actions">
        <MemoryMeter />
        <button
          className={`titlebar-running ${runningCount > 0 ? "active" : ""}`}
          onClick={openHome}
          title={`${runningCount} running — go Home`}
          aria-label={`${runningCount} running — go Home`}
        >
          <Activity size={14} strokeWidth={1.75} />
          <span>{runningCount}</span>
          <span className="titlebar-running-label">running</span>
        </button>
        {inProject && <PanelSwitch />}
      </div>
    </header>
  );
}
