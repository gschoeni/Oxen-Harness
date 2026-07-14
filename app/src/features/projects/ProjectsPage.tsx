import { useMemo, useState } from "react";
import { FolderOpen, FolderPlus, Sparkles } from "lucide-react";
import { Button } from "../../components/ui";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
import { ProjectHome } from "./ProjectHome";
import { StartProjectModal } from "./StartProjectModal";
import "./projects.css";

/** The top-level project navigator. Selecting a project opens its durable home
 * before entering chat, so guidance and context are visible rather than hidden
 * behind settings. */
export function ProjectsPage() {
  const projects = useStore((state) => state.projects);
  const sessions = useStore((state) => state.sessions);
  const runStatus = useStore((state) => state.runStatus);
  const activePath = useStore((state) => state.session?.workspace ?? null);
  const selectProject = useStore((state) => state.selectProject);
  const refreshHistory = useStore((state) => state.refreshHistory);
  const [selected, setSelected] = useState<Project | null>(null);
  const [starting, setStarting] = useState(false);

  const runningByPath = useMemo(() => {
    const counts = new Map<string, number>();
    for (const session of sessions) {
      if (runStatus[session.id] === "running") {
        counts.set(session.workspace, (counts.get(session.workspace) ?? 0) + 1);
      }
    }
    return counts;
  }, [sessions, runStatus]);

  async function openHome(project: Project) {
    setSelected(project);
    await selectProject(project.path);
  }

  async function projectChanged(project: Project) {
    setSelected((current) => ({
      ...project,
      session_count: current?.session_count ?? project.session_count,
      active: true,
    }));
    await refreshHistory();
  }

  return (
    <div className="projects-overlay" role="dialog" aria-modal="true" aria-label="Projects">
      <div className="projects-titlebar" data-tauri-drag-region />
      {selected ? (
        <ProjectHome
          project={selected}
          onBack={() => setSelected(null)}
          onProjectChanged={projectChanged}
        />
      ) : (
        <main className="projects-page">
          <header className="projects-header">
            <div>
              <div className="projects-eyebrow"><Sparkles size={14} /> Your workspaces</div>
              <h1 className="projects-title">Projects</h1>
              <p className="projects-intro">
                Give every codebase a home, a purpose, and the context your agent should carry into each chat.
              </p>
            </div>
            <Button variant="primary" className="start-project-button" onClick={() => setStarting(true)}>
              <FolderPlus size={17} /> Start a project
            </Button>
          </header>

          <section className="projects-grid" aria-label="Your projects">
            {projects.map((project) => {
              const running = runningByPath.get(project.path) ?? 0;
              return (
                <button
                  key={project.path}
                  className={`project-card ${project.path === activePath ? "active" : ""}`}
                  onClick={() => void openHome(project)}
                >
                  <span className="project-card-icon"><FolderOpen size={20} /></span>
                  <span className="project-card-main">
                    <span className="project-card-name">
                      {project.name}
                      {project.path === activePath && <span className="project-card-badge">current</span>}
                    </span>
                    <span className="project-card-description">
                      {project.description || "Add a goal and instructions for this project"}
                    </span>
                    <span className="project-card-path" title={project.path}>{project.path}</span>
                  </span>
                  <span className="project-card-meta">
                    {running > 0 && (
                      <span className="project-card-running" title={`${running} chat${running === 1 ? "" : "s"} running`}>
                        <span className="run-dot" />
                      </span>
                    )}
                    {project.context.length > 0 && (
                      <span>{project.context.length} ref{project.context.length === 1 ? "" : "s"}</span>
                    )}
                    <span>{project.session_count} chat{project.session_count === 1 ? "" : "s"}</span>
                  </span>
                </button>
              );
            })}
            {projects.length === 0 && (
              <button className="projects-empty" onClick={() => setStarting(true)}>
                <span className="projects-empty-icon"><FolderPlus size={24} /></span>
                <strong>Start your first project</strong>
                <span>Choose a folder, describe the goal, and give your agent a useful head start.</span>
              </button>
            )}
          </section>
        </main>
      )}

      {starting && (
        <StartProjectModal
          onClose={() => setStarting(false)}
          onCreated={async (project) => {
            setStarting(false);
            setSelected(project);
            await selectProject(project.path);
          }}
        />
      )}
    </div>
  );
}
