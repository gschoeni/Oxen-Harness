import { useMemo } from "react";
import { FolderOpen, FolderPlus } from "lucide-react";
import { useStore } from "../../lib/store";
import "./projects.css";

/** The full-window project picker. A project is a working directory the agent
 *  runs in; entering one scopes the sidebar to that project's chats. */
export function ProjectsPage() {
  const projects = useStore((s) => s.projects);
  const sessions = useStore((s) => s.sessions);
  const runStatus = useStore((s) => s.runStatus);
  const activePath = useStore((s) => s.session?.workspace ?? null);
  const enterProject = useStore((s) => s.enterProject);
  const createProject = useStore((s) => s.createProject);

  // How many chats are mid-run in each project, for the card indicators.
  const runningByPath = useMemo(() => {
    const counts = new Map<string, number>();
    for (const s of sessions) {
      if (runStatus[s.id] === "running") counts.set(s.workspace, (counts.get(s.workspace) ?? 0) + 1);
    }
    return counts;
  }, [sessions, runStatus]);

  return (
    <div className="projects-overlay" role="dialog" aria-modal="true" aria-label="Projects">
      <div className="projects-titlebar" data-tauri-drag-region />
      <div className="projects-page">
        <header className="projects-header">
          <div>
            <h1 className="projects-title">Projects</h1>
            <p className="projects-intro">
              A project is a folder the agent works in. Open one to chat against that codebase —
              each project keeps its own chats.
            </p>
          </div>
        </header>

        <div className="projects-grid">
          <button className="project-card new" onClick={() => createProject()}>
            <FolderPlus size={20} />
            <span className="project-card-main">
              <span className="project-card-name">Open a folder…</span>
              <span className="project-card-path">Add a project from your computer</span>
            </span>
          </button>

          {projects.map((p) => {
            const running = runningByPath.get(p.path) ?? 0;
            return (
              <button
                key={p.path}
                className={`project-card ${p.path === activePath ? "active" : ""}`}
                onClick={() => enterProject(p.path)}
              >
                <FolderOpen size={20} />
                <span className="project-card-main">
                  <span className="project-card-name">
                    {p.name}
                    {p.path === activePath && <span className="project-card-badge">current</span>}
                  </span>
                  <span className="project-card-path" title={p.path}>
                    {p.path}
                  </span>
                </span>
                <span className="project-card-meta">
                  {running > 0 && (
                    <span className="project-card-running" title={`${running} chat${running === 1 ? "" : "s"} running`}>
                      <span className="run-dot" />
                    </span>
                  )}
                  <span className="project-card-count">
                    {p.session_count} chat{p.session_count === 1 ? "" : "s"}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
