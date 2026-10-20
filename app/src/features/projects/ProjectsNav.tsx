// The left column's top-level nav: back Home, and Settings. It sits above
// the dock tab strip (Chats / Files) — column chrome, not dock content — so
// both are one click from either tab. Settings mirrors the gear on Home, so
// it's never more than a click away inside a project either.

import { useMemo } from "react";
import { ArrowLeft, Settings2 } from "lucide-react";
import { useStore } from "../../lib/store";

export function ProjectsNav() {
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const openSettings = useStore((s) => s.openSettings);
  const sessions = useStore((s) => s.sessions);
  const runStatus = useStore((s) => s.runStatus);
  const activePath = useStore(
    (s) => s.session?.workspace ?? s.projects.find((p) => p.active)?.path ?? null,
  );

  // Chats running in *other* projects still deserve a signal — a small dot
  // here says "something is happening elsewhere on the trail".
  const elsewhereBusy = useMemo(
    () =>
      sessions.some(
        (s) => s.workspace !== activePath && (runStatus[s.id] === "running" || runStatus[s.id] === "unread"),
      ),
    [sessions, runStatus, activePath],
  );

  return (
    <div className="projects-nav-row">
      <button
        className="projects-nav"
        onClick={() => setHomeOpen(true)}
        title="Home — all threads, all projects"
        aria-label="Home"
      >
        <ArrowLeft size={15} />
        <span>Home</span>
        {elsewhereBusy && <span className="projects-nav-dot" title="Activity in another project" />}
      </button>
      <button
        className="projects-nav projects-nav-settings"
        onClick={() => openSettings()}
        title="Settings (⌘,)"
        aria-label="Settings"
      >
        <Settings2 size={15} />
      </button>
    </div>
  );
}
