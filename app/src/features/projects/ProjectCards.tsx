// Home's card view: one card per project with its vital signs (running,
// needs-you, open chats). Established projects resume their newest chat;
// fresh ones open their home.

import { useMemo, useState } from "react";
import { ArrowDownAZ, Clock, FolderOpen, FolderPlus, Trash2 } from "lucide-react";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
import { getUi, setUi } from "../../lib/uiState";
import { useThreads } from "../threads/useThreads";
import { workspaceVitals } from "../threads/threads";
import { RemoveProjectModal } from "./RemoveProjectModal";

type CardSort = "recent" | "name";

function savedSort(): CardSort {
  return getUi("projectsSort") === "name" ? "name" : "recent";
}

/** Order projects for the grid: by last activity (never-used ones last) or by name. */
export function sortProjects(projects: Project[], sort: CardSort): Project[] {
  const byName = (a: Project, b: Project) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  const sorted = [...projects];
  sorted.sort(
    sort === "name" ? byName : (a, b) => (b.last_used_at ?? 0) - (a.last_used_at ?? 0) || byName(a, b),
  );
  return sorted;
}

export function ProjectCards({ onOpenProject }: { onOpenProject: (project: Project) => void }) {
  const projects = useStore((s) => s.projects);
  const sessions = useStore((s) => s.sessions);
  const activePath = useStore((s) => s.session?.workspace ?? null);
  const resume = useStore((s) => s.resume);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const selectProject = useStore((s) => s.selectProject);
  const removeProject = useStore((s) => s.removeProject);
  const [sort, setSort] = useState<CardSort>(savedSort);
  const [pendingDelete, setPendingDelete] = useState<Project | null>(null);

  function changeSort(next: CardSort) {
    setSort(next);
    setUi("projectsSort", next);
  }

  const sorted = useMemo(() => sortProjects(projects, sort), [projects, sort]);
  const threads = useThreads();

  async function openProject(project: Project) {
    // History is newest-first from the durable store; imported transcripts are
    // review-only and must never resume as an agent.
    const latest = sessions.find(
      (session) => session.workspace === project.path && session.source === "",
    );
    if (latest) {
      await resume(latest.id);
      setHomeOpen(false);
      return;
    }
    onOpenProject(project);
    await selectProject(project.path);
  }

  return (
    <>
      {projects.length > 1 && (
        <div className="projects-toolbar">
          <div className="projects-sort" role="group" aria-label="Sort projects">
            <button
              className={`projects-sort-option ${sort === "recent" ? "selected" : ""}`}
              aria-pressed={sort === "recent"}
              onClick={() => changeSort("recent")}
            >
              <Clock size={12} /> recent
            </button>
            <button
              className={`projects-sort-option ${sort === "name" ? "selected" : ""}`}
              aria-pressed={sort === "name"}
              onClick={() => changeSort("name")}
            >
              <ArrowDownAZ size={12} /> name
            </button>
          </div>
        </div>
      )}

      <section className="projects-grid" aria-label="Your projects">
        {sorted.map((project) => {
          const v = workspaceVitals(threads, project.path);
          return (
            <div
              key={project.path}
              className={`project-card ${project.path === activePath ? "active" : ""}`}
            >
              <button className="project-card-open" onClick={() => void openProject(project)}>
                <span className="project-card-icon">
                  <FolderOpen size={20} />
                </span>
                <span className="project-card-main">
                  <span className="project-card-name">
                    {project.name}
                    {project.path === activePath && <span className="project-card-badge">current</span>}
                    {v.needs > 0 && (
                      <span
                        className="project-card-attention"
                        title={`${v.needs} chat${v.needs === 1 ? "" : "s"} waiting on you`}
                      >
                        {v.needs} need{v.needs === 1 ? "s" : ""} you
                      </span>
                    )}
                  </span>
                  <span className="project-card-description">
                    {project.description || "Add a goal and instructions for this project"}
                  </span>
                  <span className="project-card-path" title={project.path}>
                    {project.path}
                  </span>
                </span>
                <span className="project-card-meta">
                  {v.running > 0 && (
                    <span className="project-card-running" title={`${v.running} running right now`}>
                      <span className="run-dot" />
                    </span>
                  )}
                  {v.open > 0 && <span>{v.open} open</span>}
                  <span>
                    {project.session_count} chat{project.session_count === 1 ? "" : "s"}
                  </span>
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
          <div className="projects-empty">
            <span className="projects-empty-icon">
              <FolderPlus size={24} />
            </span>
            <strong>Start your first project</strong>
            <span>Choose a folder, describe the goal, and give your agent a useful head start.</span>
          </div>
        )}
      </section>

      {pendingDelete && (
        <RemoveProjectModal
          name={pendingDelete.name}
          onCancel={() => setPendingDelete(null)}
          onConfirm={async () => {
            await removeProject(pendingDelete.path);
            setPendingDelete(null);
          }}
        />
      )}
    </>
  );
}
