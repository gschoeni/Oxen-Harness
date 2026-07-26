// The home's card view — the calm, project-first lens the trails view trades
// away. One card per project with its vital signs from the board (running,
// needs-you, open threads), for days when you want altitude, not detail.
// Established projects resume their newest chat; fresh ones open their home.

import { useMemo, useState } from "react";
import { ArrowDownAZ, Clock, FolderOpen, FolderPlus, Trash2 } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
import { getUi, setUi } from "../../lib/uiState";
import type { Board } from "./ledger";

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

/** A workspace's vital signs, folded from the board's full thread lists. */
interface Vitals {
  open: number;
  running: number;
  needs: number;
}

export function ProjectCards({
  board,
  onOpenProject,
}: {
  board: Board;
  onOpenProject: (project: Project) => void;
}) {
  const projects = useStore((s) => s.projects);
  const sessions = useStore((s) => s.sessions);
  const activePath = useStore((s) => s.session?.workspace ?? null);
  const resume = useStore((s) => s.resume);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const selectProject = useStore((s) => s.selectProject);
  const removeProject = useStore((s) => s.removeProject);
  const [sort, setSort] = useState<CardSort>(savedSort);
  const [pendingDelete, setPendingDelete] = useState<Project | null>(null);
  const [deleting, setDeleting] = useState(false);

  function changeSort(next: CardSort) {
    setSort(next);
    setUi("projectsSort", next);
  }

  const sorted = useMemo(() => sortProjects(projects, sort), [projects, sort]);

  const vitals = useMemo(() => {
    const map = new Map<string, Vitals>();
    for (const train of board.trains) {
      map.set(train.workspace, {
        open: train.threads.length,
        running: train.threads.filter((t) => t.state === "running").length,
        needs: train.threads.filter((t) => t.need !== null).length,
      });
    }
    return map;
  }, [board]);

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

  return (
    <>
      {projects.length > 1 && (
        <div className="ledger-cards-toolbar">
          <div className="ledger-sort" role="group" aria-label="Sort projects">
            <button
              className={`ledger-sort-option ${sort === "recent" ? "selected" : ""}`}
              aria-pressed={sort === "recent"}
              onClick={() => changeSort("recent")}
            >
              <Clock size={12} /> recent
            </button>
            <button
              className={`ledger-sort-option ${sort === "name" ? "selected" : ""}`}
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
          const v = vitals.get(project.path);
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
                  </span>
                  <span className="project-card-description">
                    {project.description || "Add a goal and instructions for this project"}
                  </span>
                  <span className="project-card-path" title={project.path}>
                    {project.path}
                  </span>
                </span>
                <span className="project-card-meta">
                  {(v?.running ?? 0) > 0 && (
                    <span className="project-card-running" title={`${v?.running} riding right now`}>
                      <span className="run-dot" />
                    </span>
                  )}
                  {(v?.needs ?? 0) > 0 && (
                    <span className="project-card-needs">{v?.needs} need you</span>
                  )}
                  {(v?.open ?? 0) > 0 && <span>{v?.open} on the trail</span>}
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
    </>
  );
}
