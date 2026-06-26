import { useEffect, useMemo, useState } from "react";
import { ChevronRight, FolderPlus, Plus, Settings as SettingsIcon, Trash2 } from "lucide-react";
import { useStore } from "../../lib/store";
import { relativeTime } from "../../lib/format";
import { Button, Modal } from "../../components/ui";
import type { Project, RunStatus, SessionSummary } from "../../lib/types";
import "./sidebar.css";

/** One project's chats. */
interface Group {
  project: Project;
  rows: SessionSummary[];
}

export function Sidebar() {
  const theme = useStore((s) => s.theme);
  const sessions = useStore((s) => s.sessions);
  const projects = useStore((s) => s.projects);
  const session = useStore((s) => s.session);
  const runStatus = useStore((s) => s.runStatus);
  const startNewSession = useStore((s) => s.startNewSession);
  const enterProject = useStore((s) => s.enterProject);
  const resume = useStore((s) => s.resume);
  const removeSession = useStore((s) => s.removeSession);
  const setProjectsOpen = useStore((s) => s.setProjectsOpen);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);

  // The chat queued for deletion (drives the confirm modal), and whether the
  // delete request is in flight.
  const [pendingDelete, setPendingDelete] = useState<SessionSummary | null>(null);
  const [deleting, setDeleting] = useState(false);

  async function confirmDelete() {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await removeSession(pendingDelete.id);
      setPendingDelete(null);
    } finally {
      setDeleting(false);
    }
  }

  const icon = theme?.voice.prompt_icon || "🐂";
  const currentId = session?.session_id ?? null;
  const activePath = session?.workspace ?? projects.find((p) => p.active)?.path ?? null;

  // Group chats under their project (working directory). Every known project
  // shows even when empty; a brand-new untitled chat is pinned into its project.
  const groups = useMemo<Group[]>(() => {
    const rowsByPath = new Map<string, SessionSummary[]>();
    for (const p of projects) rowsByPath.set(p.path, []);
    for (const s of sessions) {
      if (!rowsByPath.has(s.workspace)) rowsByPath.set(s.workspace, []);
      rowsByPath.get(s.workspace)!.push(s);
    }
    // The active chat may be new (no title/row yet) — surface it in its project.
    if (currentId && session && !sessions.some((s) => s.id === currentId)) {
      const list = rowsByPath.get(session.workspace) ?? [];
      list.unshift({
        id: currentId,
        workspace: session.workspace,
        model: session.model,
        created_at: 0,
        title: null,
        message_count: 0,
      });
      rowsByPath.set(session.workspace, list);
    }
    // Keep the backend's order (active first, busiest), then any extra paths.
    const known = new Map(projects.map((p) => [p.path, p]));
    const ordered: Group[] = projects.map((p) => ({ project: p, rows: rowsByPath.get(p.path) ?? [] }));
    for (const [path, rows] of rowsByPath) {
      if (!known.has(path)) {
        ordered.push({ project: { path, name: path.split("/").pop() || path, session_count: rows.length, active: path === activePath }, rows });
      }
    }
    return ordered;
  }, [projects, sessions, session, currentId, activePath]);

  // Which folders are expanded — the active project opens by default.
  const [open, setOpen] = useState<Set<string>>(() => new Set(activePath ? [activePath] : []));
  useEffect(() => {
    if (activePath) setOpen((s) => (s.has(activePath) ? s : new Set(s).add(activePath)));
  }, [activePath]);
  const toggle = (path: string) =>
    setOpen((s) => {
      const next = new Set(s);
      next.has(path) ? next.delete(path) : next.add(path);
      return next;
    });

  return (
    <aside className="sidebar">
      {/* Transparent strip over the traffic-light row so the empty space above
          the brand drags the window (overlay title bar). */}
      <div className="sidebar-titlebar" data-tauri-drag-region />
      <div className="brand" data-tauri-drag-region>
        <span className="brand-icon">{icon}</span>
        <span>oxen-harness</span>
      </div>

      <button className="new-chat" onClick={() => startNewSession()}>
        <Plus size={17} />
        New chat
      </button>

      <div className="history-head">
        <span>Projects</span>
        <button
          className="head-action"
          onClick={() => setProjectsOpen(true)}
          title="Manage projects"
          aria-label="Manage projects"
        >
          <FolderPlus size={15} />
        </button>
      </div>

      <div className="history">
        {groups.length === 0 ? (
          <div className="history-empty">No projects yet.</div>
        ) : (
          groups.map(({ project, rows }) => (
            <ProjectFolder
              key={project.path}
              project={project}
              rows={rows}
              active={project.path === activePath}
              expanded={open.has(project.path)}
              currentId={currentId}
              runStatus={runStatus}
              onToggle={() => toggle(project.path)}
              onNewChat={() => enterProject(project.path)}
              onOpenChat={resume}
              onDeleteChat={setPendingDelete}
            />
          ))
        )}
      </div>

      <div className="sidebar-foot">
        <button className="foot-btn" onClick={() => setSettingsOpen(true)}>
          <SettingsIcon size={17} />
          Settings
        </button>
      </div>

      {pendingDelete && (
        <Modal title="Delete chat?" onClose={() => !deleting && setPendingDelete(null)}>
          <p className="delete-confirm-text">
            Permanently delete{" "}
            <strong>{pendingDelete.title?.trim() || "this chat"}</strong> and its messages? This
            can’t be undone.
          </p>
          <div className="delete-confirm-actions">
            <Button variant="ghost" onClick={() => setPendingDelete(null)} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="danger" onClick={confirmDelete} disabled={deleting}>
              {deleting ? "Deleting…" : "Delete"}
            </Button>
          </div>
        </Modal>
      )}
    </aside>
  );
}

/** A collapsible project folder with its nested chats. */
function ProjectFolder({
  project,
  rows,
  active,
  expanded,
  currentId,
  runStatus,
  onToggle,
  onNewChat,
  onOpenChat,
  onDeleteChat,
}: {
  project: Project;
  rows: SessionSummary[];
  active: boolean;
  expanded: boolean;
  currentId: string | null;
  runStatus: Record<string, RunStatus>;
  onToggle: () => void;
  onNewChat: () => void;
  onOpenChat: (id: string) => void;
  onDeleteChat: (row: SessionSummary) => void;
}) {
  return (
    <div className={`project ${active ? "active" : ""}`}>
      <div className="project-head">
        <button className="project-toggle" onClick={onToggle} aria-expanded={expanded}>
          <ChevronRight className={`project-chevron ${expanded ? "open" : ""}`} size={14} />
          <span className="project-name" title={project.path}>
            {project.name}
          </span>
          {project.session_count > 0 && <span className="project-count">{project.session_count}</span>}
        </button>
        <button
          className="head-action"
          onClick={onNewChat}
          title={`New chat in ${project.name}`}
          aria-label={`New chat in ${project.name}`}
        >
          <Plus size={14} />
        </button>
      </div>
      {expanded && (
        <div className="project-chats">
          {rows.length === 0 ? (
            <div className="project-empty">No chats yet</div>
          ) : (
            rows.map((s) => (
              <ChatRow
                key={s.id}
                row={s}
                current={s.id === currentId}
                status={runStatus[s.id]}
                onOpen={() => onOpenChat(s.id)}
                onDelete={() => onDeleteChat(s)}
              />
            ))
          )}
        </div>
      )}
    </div>
  );
}

/** A single chat entry with its run indicator and a hover-revealed delete icon. */
function ChatRow({
  row,
  current,
  status,
  onOpen,
  onDelete,
}: {
  row: SessionSummary;
  current: boolean;
  status: RunStatus | undefined;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const title = row.title?.trim() || "New chat";
  return (
    <div className={`history-item ${current ? "active" : ""}`}>
      <button className="history-open" onClick={onOpen}>
        <span className="history-text">
          <span className="history-title">{title}</span>
          <span className="history-sub">
            {row.created_at ? relativeTime(row.created_at) : "Not started yet"}
          </span>
        </span>
        {status === "running" ? (
          <span className="chat-status running" title="Running" aria-label="Running">
            <span className="run-dot" />
          </span>
        ) : status === "unread" && !current ? (
          <span className="chat-status unread" title="Done — unread" aria-label="Done, unread" />
        ) : null}
      </button>
      <button
        className="history-delete"
        title="Delete chat"
        aria-label={`Delete chat: ${title}`}
        onClick={(e) => {
          e.stopPropagation();
          onDelete();
        }}
      >
        <Trash2 size={14} />
      </button>
    </div>
  );
}
