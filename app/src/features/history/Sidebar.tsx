import { useEffect, useMemo, useState, type PointerEvent } from "react";
import { FolderOpen, Plus, Settings as SettingsIcon, Trash2 } from "lucide-react";
import { useStore } from "../../lib/store";
import { relativeTime } from "../../lib/format";
import { Button, Modal } from "../../components/ui";
import { DockToggle } from "../docks/DockToggle";
import { needLabel, needRank, needsUser, rankOfNeed, type Thread } from "../ledger/ledger";
import { useBoard } from "../ledger/useBoard";
import type { RunStatus, SessionSummary } from "../../lib/types";
import "./sidebar.css";

export function Sidebar({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const sessions = useStore((s) => s.sessions);
  const projects = useStore((s) => s.projects);
  const session = useStore((s) => s.session);
  const runStatus = useStore((s) => s.runStatus);
  const startNewSession = useStore((s) => s.startNewSession);
  const resume = useStore((s) => s.resume);
  const removeSession = useStore((s) => s.removeSession);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const ledger = useStore((s) => s.ledger);
  const refreshLedger = useStore((s) => s.refreshLedger);

  // The chat list reads the same derived board as the home cards, so the
  // "N need you" a card promised is exactly the rows sectioned off up top.
  // A chat can be reached without ever visiting Home — fetch the snapshot
  // once if nothing has; the store keeps it fresh from there.
  useEffect(() => {
    if (!ledger) void refreshLedger();
  }, [ledger, refreshLedger]);
  const board = useBoard();
  // Every thread the board has a verdict on, by session. Archived ("lost")
  // threads are left out on purpose: the amnesty means they never nag, and
  // the home card doesn't count them either.
  const threads = useMemo(() => {
    const map = new Map<string, Thread>();
    for (const train of board?.trains ?? []) {
      for (const thread of train.threads) map.set(thread.entry.id, thread);
    }
    for (const thread of board?.settled ?? []) map.set(thread.entry.id, thread);
    return map;
  }, [board]);

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

  const currentId = session?.session_id ?? null;
  const activePath = session?.workspace ?? projects.find((p) => p.active)?.path ?? null;
  const activeProject = projects.find((p) => p.path === activePath) ?? null;
  const projectName = activeProject?.name ?? (activePath ? activePath.split("/").pop() || activePath : null);

  // The sidebar shows only persisted chats from the current project;
  // everything else lives on the Projects page. Imported transcripts (Claude
  // Code / Cursor) are review-only — they live in Settings → Training data,
  // not here, so they can't be resumed as live agents.
  const listed = activePath
    ? sessions.filter((s) => s.workspace === activePath && s.source === "")
    : [];
  // A brand-new chat has no user turn yet, so the persisted list can't show
  // it — pin it on top until its first message lands. Without this the open
  // chat has no row and no highlight, and clicking any listed chat navigates
  // away with no way back.
  const pinned: SessionSummary | null =
    session && session.workspace === activePath && !listed.some((s) => s.id === session.session_id)
      ? {
          id: session.session_id,
          workspace: session.workspace,
          model: session.model,
          created_at: 0,
          title: null,
          message_count: 0,
          review_status: "",
          source: "",
        }
      : null;
  const rows = pinned ? [pinned, ...listed] : listed;
  // Pressing "+" while already on that fresh, unstarted chat stays put —
  // minting another empty session each press would only pile up orphans.
  const onFreshChat = pinned !== null && runStatus[pinned.id] !== "running";

  const sections = useMemo(() => sectionRows(rows, threads, runStatus), [rows, threads, runStatus]);

  return (
    <aside className="sidebar">
      {onResizeStart && (
        <div
          className="sidebar-resizer"
          onPointerDown={onResizeStart}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize chat list"
        />
      )}
      {activePath ? (
        <>
          <div className="current-project" title={activePath}>
            <FolderOpen size={18} />
            <span className="current-project-name">{projectName}</span>
            {/* The panel control lives here — by the project title, where the
                user looks for it — not on the draggable edge. */}
            <DockToggle side="left" />
          </div>

          <button
            className="new-chat"
            onClick={() => {
              if (!onFreshChat) void startNewSession();
            }}
          >
            <Plus size={18} />
            New chat
          </button>

          <div className="history">
            {rows.length === 0 ? (
              <>
                <div className="history-head">
                  <span>Chats</span>
                </div>
                <div className="history-empty">No chats yet. Start one above.</div>
              </>
            ) : (
              <>
                {sections.needs.length > 0 && (
                  <>
                    <div
                      className="history-head needs"
                      title="Loose ends: pick each one back up, or open it and tie the knot"
                    >
                      <span>Needs you</span>
                      <span className="history-count">{sections.needs.length}</span>
                    </div>
                    {sections.needs.map((s) => (
                      <ChatRow
                        key={s.row.id}
                        row={s.row}
                        current={s.row.id === currentId}
                        status={runStatus[s.row.id]}
                        need={s.label}
                        onOpen={() => resume(s.row.id)}
                        onDelete={() => setPendingDelete(s.row)}
                      />
                    ))}
                  </>
                )}
                {sections.open.length > 0 && (
                  <div className="history-head">
                    <span>{sections.needs.length > 0 ? "Other chats" : "Chats"}</span>
                  </div>
                )}
                {sections.open.map((s) => (
                  <ChatRow
                    key={s.id}
                    row={s}
                    current={s.id === currentId}
                    status={runStatus[s.id]}
                    onOpen={() => resume(s.id)}
                    onDelete={() => setPendingDelete(s)}
                  />
                ))}
                {sections.settled.length > 0 && (
                  <div className="history-head settled" title="Tied off — nothing owed here">
                    <span>Settled</span>
                    <span className="history-count">{sections.settled.length}</span>
                  </div>
                )}
                {sections.settled.map((s) => (
                  <ChatRow
                    key={s.id}
                    row={s}
                    current={s.id === currentId}
                    status={runStatus[s.id]}
                    settled
                    onOpen={() => resume(s.id)}
                    onDelete={() => setPendingDelete(s)}
                  />
                ))}
              </>
            )}
          </div>
        </>
      ) : (
        <>
          {/* No project yet — keep the panel control reachable in the same
              band it lives in once a project is open. */}
          <div className="current-project">
            <span className="current-project-name">No project</span>
            <DockToggle side="left" />
          </div>
          <div className="history">
            <div className="history-empty">
              No project open. Pick one from the Projects page to start chatting.
            </div>
          </div>
        </>
      )}

      <div className="sidebar-foot">
        <button className="foot-btn" onClick={() => setSettingsOpen(true)}>
          <SettingsIcon size={18} />
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

/** A needy chat with the words it wears. */
interface NeedyRow {
  row: SessionSummary;
  label: string;
}

/** The project's chats in three bands, top to bottom: the ones that need the
 *  user (most urgent first — a parked agent, a lost reply, a finish they
 *  haven't seen, an open plan, a thread going cold), the rest of the open
 *  chats in their usual order, and the tied-off ones last. Reads the board's
 *  verdict per thread; a chat the board hasn't met yet (no user turn, or the
 *  snapshot still loading) is an ordinary open chat. A finish the store saw
 *  land offscreen this session (`unread`) counts as needing the user even
 *  before the board catches up — the dot always meant "look at this". */
export function sectionRows(
  rows: SessionSummary[],
  threads: Map<string, Thread>,
  runStatus: Record<string, RunStatus | undefined>,
): { needs: NeedyRow[]; open: SessionSummary[]; settled: SessionSummary[] } {
  const needs: (NeedyRow & { rank: number; index: number })[] = [];
  const open: SessionSummary[] = [];
  const settled: SessionSummary[] = [];
  rows.forEach((row, index) => {
    const thread = threads.get(row.id);
    if (thread?.state === "settled") {
      settled.push(row);
    } else if (thread && needsUser(thread)) {
      needs.push({ row, label: needLabel(thread) ?? "", rank: needRank(thread), index });
    } else if (runStatus[row.id] === "unread") {
      needs.push({ row, label: "finished while you were away", rank: rankOfNeed("finished"), index });
    } else {
      open.push(row);
    }
  });
  needs.sort((a, b) => a.rank - b.rank || a.index - b.index);
  return { needs: needs.map(({ row, label }) => ({ row, label })), open, settled };
}

/** A single chat entry with its run indicator and a hover-revealed delete icon.
 *  A needy row also carries its reason under the title, in the warning color
 *  the home card's pill wears; a settled row is quieted. */
function ChatRow({
  row,
  current,
  status,
  need,
  settled = false,
  onOpen,
  onDelete,
}: {
  row: SessionSummary;
  current: boolean;
  status: RunStatus | undefined;
  need?: string;
  settled?: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const title = row.title?.trim() || "New chat";
  const model = shortModel(row.model);
  const when = row.created_at ? relativeTime(row.created_at) : "Not started yet";
  // A live dev server for this chat: show its port so the running app is
  // findable from the sidebar.
  const previewPort = useStore((s) => {
    const p = s.previews[row.id];
    return p && p.phase === "ready" ? p.port : null;
  });
  return (
    <div
      className={`history-item ${current ? "active" : ""} ${need ? "needy" : ""} ${settled ? "settled" : ""}`}
    >
      <button className="history-open" onClick={onOpen}>
        <span className="history-text">
          <span className="history-title">{title}</span>
          {need && <span className="history-need">{need}</span>}
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
            {previewPort != null && (
              <>
                <span className="history-sep">·</span>
                <span className="history-preview" title={`Dev server on port ${previewPort}`}>
                  🌐:{previewPort}
                </span>
              </>
            )}
          </span>
        </span>
        {status === "running" ? (
          <span className="chat-status running" title="Running" aria-label="Running">
            <span className="run-dot" />
          </span>
        ) : need ? (
          <span className="chat-status needy" title={need} aria-label="Needs you" />
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
        <Trash2 size={15} />
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
