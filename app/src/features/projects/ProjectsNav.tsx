// The left column's top-level nav: back to the Projects page. It sits above
// the dock tab strip (Chats / Files) — column chrome, not dock content — so
// the way back to Projects is one click from either tab.

import { useMemo } from "react";
import { ArrowLeft } from "lucide-react";
import { useStore } from "../../lib/store";

export function ProjectsNav() {
  const setProjectsOpen = useStore((s) => s.setProjectsOpen);
  const sessions = useStore((s) => s.sessions);
  const runStatus = useStore((s) => s.runStatus);
  const activePath = useStore(
    (s) => s.session?.workspace ?? s.projects.find((p) => p.active)?.path ?? null,
  );

  // Chats running in *other* projects still deserve a signal — a small dot
  // here says "something is happening elsewhere".
  const elsewhereBusy = useMemo(
    () =>
      sessions.some(
        (s) => s.workspace !== activePath && (runStatus[s.id] === "running" || runStatus[s.id] === "unread"),
      ),
    [sessions, runStatus, activePath],
  );

  return (
    <button
      className="projects-nav"
      onClick={() => setProjectsOpen(true)}
      title="All projects"
      aria-label="All projects"
    >
      <ArrowLeft size={15} />
      <span>Projects</span>
      {elsewhereBusy && <span className="projects-nav-dot" title="Activity in another project" />}
    </button>
  );
}
