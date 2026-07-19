import { useMemo, useState } from "react";
import { ArrowDownAZ, Clock, FolderOpen, FolderPlus, Sparkles, Trash2 } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
import { ProjectHome } from "./ProjectHome";
import { StartProjectModal } from "./StartProjectModal";
import "./projects.css";

type ProjectSort = "recent" | "name";

const SORT_STORAGE_KEY = "oxen-harness.projects-sort";

function savedSort(): ProjectSort {
  return localStorage.getItem(SORT_STORAGE_KEY) === "name" ? "name" : "recent";
}

/** Order projects for the grid: by last activity (never-used ones last) or by name. */
export function sortProjects(projects: Project[], sort: ProjectSort): Project[] {
  const byName = (a: Project, b: Project) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  const sorted = [...projects];
  sorted.sort(sort === "name" ? byName : (a, b) => (b.last_used_at ?? 0) - (a.last_used_at ?? 0) || byName(a, b));
  return sorted;
}

/** The top-level project navigator. Established projects resume their newest
 * chat; projects without history open the getting-started surface. */
export function ProjectsPage() {
  const projects = useStore((state) => state.projects);
  const sessions = useStore((state) => state.sessions);
  const runStatus = useStore((state) => state.runStatus);
  const activePath = useStore((state) => state.session?.workspace ?? null);
  const selectProject = useStore((state) => state.selectProject);
  const enterProject = useStore((state) => state.enterProject);
  const resume = useStore((state) => state.resume);
  const setProjectsOpen = useStore((state) => state.setProjectsOpen);
  const projectHomePath = useStore((state) => state.projectHomePath);
  const refreshHistory = useStore((state) => state.refreshHistory);
  const removeProject = useStore((state) => state.removeProject);
  const [selected, setSelected] = useState<Project | null>(() =>
    projectHomePath ? projects.find((project) => project.path === projectHomePath) ?? null : null,
  );
  const [starting, setStarting] = useState(false);
  const [sort, setSort] = useState<ProjectSort>(savedSort);
  // The project queued for removal (drives the confirm modal), and whether the
  // request is in flight.
  const [pendingDelete, setPendingDelete] = useState<Project | null>(null);
  const [deleting, setDeleting] = useState(false);

  async function confirmDelete() {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await removeProject(pendingDelete.path);
      setPendingDelete(null);
    } finally {
      setDeleting(false);
    }
  }

  function changeSort(next: ProjectSort) {
    setSort(next);
    localStorage.setItem(SORT_STORAGE_KEY, next);
  }

  const sorted = useMemo(() => sortProjects(projects, sort), [projects, sort]);

  const runningByPath = useMemo(() => {
    const counts = new Map<string, number>();
    for (const session of sessions) {
      if (runStatus[session.id] === "running") {
        counts.set(session.workspace, (counts.get(session.workspace) ?? 0) + 1);
      }
    }
    return counts;
  }, [sessions, runStatus]);

  async function openProject(project: Project) {
    // History is returned newest-first by the durable session store. Use it
    // directly so navigation cannot drift from a separately derived count.
    // Imported transcripts are review-only and must never resume as an agent.
    const latest = sessions.find(
      (session) => session.workspace === project.path && session.source === "",
    );
    if (latest) {
      await resume(latest.id);
      setProjectsOpen(false);
      return;
    }
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

          {projects.length > 1 && (
            <div className="projects-toolbar">
              <div className="projects-sort" role="group" aria-label="Sort projects">
                <button
                  className={`projects-sort-option ${sort === "recent" ? "selected" : ""}`}
                  aria-pressed={sort === "recent"}
                  onClick={() => changeSort("recent")}
                >
                  <Clock size={13} /> Recent
                </button>
                <button
                  className={`projects-sort-option ${sort === "name" ? "selected" : ""}`}
                  aria-pressed={sort === "name"}
                  onClick={() => changeSort("name")}
                >
                  <ArrowDownAZ size={13} /> Name
                </button>
              </div>
            </div>
          )}

          <section className="projects-grid" aria-label="Your projects">
            {sorted.map((project) => {
              const running = runningByPath.get(project.path) ?? 0;
              return (
                <div
                  key={project.path}
                  className={`project-card ${project.path === activePath ? "active" : ""}`}
                >
                  <button className="project-card-open" onClick={() => void openProject(project)}>
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
                      {project.last_used_at != null && (
                        <span className="project-card-used">{relativeTime(project.last_used_at)}</span>
                      )}
                    </span>
                  </button>
                  <button
                    className="project-card-delete"
                    title="Remove project"
                    aria-label={`Remove project: ${project.name}`}
                    onClick={() => setPendingDelete(project)}
                  >
                    <Trash2 size={15} />
                  </button>
                </div>
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
            // A brand-new project drops straight into a fresh chat; its home
            // page (goal, instructions, context) stays reachable from the grid.
            await enterProject(project.path);
          }}
        />
      )}

      {pendingDelete && (
        <Modal title="Remove project?" onClose={() => !deleting && setPendingDelete(null)}>
          <p className="delete-confirm-text">
            Remove <strong>{pendingDelete.name}</strong> from your projects? Its folder and chat
            history stay on disk — it just won’t be listed here anymore.
          </p>
          <div className="delete-confirm-actions">
            <Button variant="ghost" onClick={() => setPendingDelete(null)} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="danger" onClick={confirmDelete} disabled={deleting}>
              {deleting ? "Removing…" : "Remove"}
            </Button>
          </div>
        </Modal>
      )}
    </div>
  );
}
