import { useMemo, useState } from "react";
import { ArrowLeft, FolderOpen, Plus, Settings as SettingsIcon, Trash2 } from "lucide-react";
import { useStore } from "../../lib/store";
import { relativeTime } from "../../lib/format";
import { Button, Modal } from "../../components/ui";
import type { RunStatus, SessionSummary } from "../../lib/types";
import "./sidebar.css";

export function Sidebar() {
  const theme = useStore((s) => s.theme);
  const sessions = useStore((s) => s.sessions);
  const projects = useStore((s) => s.projects);
  const session = useStore((s) => s.session);
  const runStatus = useStore((s) => s.runStatus);
  const startNewSession = useStore((s) => s.startNewSession);
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
  const activeProject = projects.find((p) => p.path === activePath) ?? null;
  const projectName = activeProject?.name ?? (activePath ? activePath.split("/").pop() || activePath : null);

  // The sidebar shows only the current project's chats; everything else lives
  // on the Projects page. A brand-new untitled chat is pinned to the top.
  const rows = useMemo<SessionSummary[]>(() => {
    if (!activePath) return [];
    const list = sessions.filter((s) => s.workspace === activePath);
    if (currentId && session && session.workspace === activePath && !list.some((s) => s.id === currentId)) {
      list.unshift({
        id: currentId,
        workspace: session.workspace,
        model: session.model,
        created_at: 0,
        title: null,
        message_count: 0,
        review_status: "",
      });
    }
    return list;
  }, [sessions, session, currentId, activePath]);

  // Chats running in *other* projects still deserve a signal — a small dot on
  // the Projects link says "something is happening elsewhere".
  const elsewhereBusy = useMemo(
    () =>
      sessions.some(
        (s) => s.workspace !== activePath && (runStatus[s.id] === "running" || runStatus[s.id] === "unread"),
      ),
    [sessions, runStatus, activePath],
  );

  return (
    <aside className="sidebar">
      {/* Transparent strip over the traffic-light row so the empty space above
          the brand drags the window (overlay title bar). */}
      <div className="sidebar-titlebar" data-tauri-drag-region />
      <div className="brand" data-tauri-drag-region>
        <span className="brand-icon">{icon}</span>
        <span>oxen-harness</span>
      </div>

      <button
        className="projects-link"
        onClick={() => setProjectsOpen(true)}
        title="All projects"
        aria-label="All projects"
      >
        <ArrowLeft size={15} />
        <span>Projects</span>
        {elsewhereBusy && <span className="projects-link-dot" title="Activity in another project" />}
      </button>

      {activePath ? (
        <>
          <div className="current-project" title={activePath}>
            <FolderOpen size={17} />
            <span className="current-project-name">{projectName}</span>
          </div>

          <button className="new-chat" onClick={() => startNewSession()}>
            <Plus size={17} />
            New chat
          </button>

          <div className="history-head">
            <span>Chats</span>
          </div>

          <div className="history">
            {rows.length === 0 ? (
              <div className="history-empty">No chats yet. Start one above.</div>
            ) : (
              rows.map((s) => (
                <ChatRow
                  key={s.id}
                  row={s}
                  current={s.id === currentId}
                  status={runStatus[s.id]}
                  onOpen={() => resume(s.id)}
                  onDelete={() => setPendingDelete(s)}
                />
              ))
            )}
          </div>
        </>
      ) : (
        <div className="history">
          <div className="history-empty">
            No project open. Pick one from the Projects page to start chatting.
          </div>
        </div>
      )}

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
  const model = shortModel(row.model);
  const when = row.created_at ? relativeTime(row.created_at) : "Not started yet";
  return (
    <div className={`history-item ${current ? "active" : ""}`}>
      <button className="history-open" onClick={onOpen}>
        <span className="history-text">
          <span className="history-title">{title}</span>
          <span className="history-sub">
            <span className="history-date">{when}</span>
            {model && (
              <>
                <span className="history-sep">·</span>
                <span className="history-model" title={row.model}>
                  {model}
                </span>
              </>
            )}
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

/** A compact model label for the sidebar: drop the provider prefix and any
 *  date suffix so `anthropic/claude-sonnet-4-5-20250929` reads as
 *  `claude-sonnet-4-5`. */
function shortModel(model: string): string {
  const id = (model ?? "").trim();
  if (!id) return "";
  const name = id.split("/").pop() ?? id;
  return name.replace(/-\d{6,8}$/, "");
}
