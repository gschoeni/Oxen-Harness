// The history: every chat, searchable, one click from being a tab again.
//
// It opens from the clock at the end of the tab strip (or ⌘K), lands the
// cursor in the search box, and closes the moment a chat is picked — the
// chosen chat becomes the visible tab. Scoped to the current project by
// default, with every project one toggle away. Rows keep the sidebar's old
// order: what needs you first, then the open chats, then the finished ones.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Search, Trash2 } from "lucide-react";
import { useStore } from "../../lib/store";
import { basename, relativeTime, shortModel } from "../../lib/format";
import { Modal } from "../../components/ui";
import type { SessionSummary } from "../../lib/types";
import { useThreads } from "../threads/useThreads";
import { StatusDot } from "../tabs/StatusDot";
import { tabSignals, tabStatus, type TabStatus } from "../tabs/tabStatus";
import { DeleteChatModal } from "./DeleteChatModal";
import { matchesQuery, sectionRows, type Sections } from "./sections";
import "./history.css";

type Scope = "project" | "all";

export function HistoryModal() {
  const setHistoryOpen = useStore((s) => s.setHistoryOpen);
  const sessions = useStore((s) => s.sessions);
  const projects = useStore((s) => s.projects);
  const session = useStore((s) => s.session);
  const runStatus = useStore((s) => s.runStatus);
  const approvals = useStore((s) => s.approvals);
  const question = useStore((s) => s.question);
  const chatTabs = useStore((s) => s.chatTabs);
  const resume = useStore((s) => s.resume);
  const homeOpen = useStore((s) => s.homeOpen);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const addNotice = useStore((s) => s.addNotice);
  const threads = useThreads();

  const workspace = session?.workspace ?? projects.find((p) => p.active)?.path ?? null;
  const [scope, setScope] = useState<Scope>(workspace ? "project" : "all");
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const [pendingDelete, setPendingDelete] = useState<SessionSummary | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const names = useMemo(() => new Map(projects.map((p) => [p.path, p.name])), [projects]);
  const projectName = (path: string) => names.get(path) ?? basename(path);

  // Imported transcripts (Claude Code / Cursor) are review-only — they live
  // in Settings → Training data and can't be resumed as live agents.
  const sections: Sections = useMemo(() => {
    const listed = sessions.filter(
      (s) => s.source === "" && (scope === "all" || s.workspace === workspace),
    );
    const matched = listed.filter((s) =>
      matchesQuery(s, names.get(s.workspace) ?? basename(s.workspace), query),
    );
    return sectionRows(matched, threads, runStatus);
  }, [sessions, scope, workspace, query, threads, runStatus, names]);

  // The rows in reading order, for the arrow keys.
  const flat = useMemo(
    () => [...sections.needs.map((n) => n.row), ...sections.open, ...sections.finished],
    [sections],
  );
  const highlighted = flat[Math.min(cursor, Math.max(0, flat.length - 1))]?.id;

  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => setCursor(0), [query, scope]);
  // Escape closes from anywhere in the modal — the scope toggle, a delete
  // button — not only from the search box.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") setHistoryOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setHistoryOpen]);
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(".history-item.highlighted");
    el?.scrollIntoView?.({ block: "nearest" });
  }, [highlighted]);

  function open(id: string) {
    setHistoryOpen(false);
    resume(id)
      .then(() => {
        // Picked from Home: leave for the chat.
        if (homeOpen) setHomeOpen(false);
      })
      .catch((e: unknown) => addNotice(`Couldn't open that chat: ${String(e)}`));
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, Math.max(0, flat.length - 1)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
    } else if (e.key === "Enter") {
      if (highlighted) open(highlighted);
    }
  }

  const statusOf = (id: string): TabStatus =>
    tabStatus(tabSignals({ runStatus, approvals, question }, id, threads.get(id)));
  const openTabs = useMemo(() => new Set(Object.values(chatTabs).flat()), [chatTabs]);
  const currentId = session?.session_id ?? null;
  const empty = flat.length === 0;

  const row = (s: SessionSummary, need?: string, finished = false) => (
    <HistoryRow
      key={s.id}
      row={s}
      status={statusOf(s.id)}
      need={need}
      finished={finished}
      current={s.id === currentId}
      highlighted={s.id === highlighted}
      asTab={openTabs.has(s.id)}
      project={scope === "all" ? projectName(s.workspace) : null}
      onOpen={() => open(s.id)}
      onHover={() => setCursor(flat.indexOf(s))}
      onDelete={() => setPendingDelete(s)}
    />
  );

  return (
    <Modal title="Chats" onClose={() => setHistoryOpen(false)} wide>
      <div className="history-modal" onKeyDown={onKeyDown}>
        <div className="history-controls">
          <label className="history-search">
            <Search size={15} aria-hidden="true" />
            <input
              ref={inputRef}
              type="search"
              placeholder="Search chats…"
              aria-label="Search chats"
              value={query}
              spellCheck={false}
              autoComplete="off"
              onChange={(e) => setQuery(e.target.value)}
            />
          </label>
          {workspace && (
            <div className="history-scope" role="radiogroup" aria-label="Which chats">
              <button
                role="radio"
                aria-checked={scope === "project"}
                className={`history-scope-btn${scope === "project" ? " active" : ""}`}
                onClick={() => setScope("project")}
              >
                {projectName(workspace)}
              </button>
              <button
                role="radio"
                aria-checked={scope === "all"}
                className={`history-scope-btn${scope === "all" ? " active" : ""}`}
                onClick={() => setScope("all")}
              >
                All projects
              </button>
            </div>
          )}
        </div>

        <div className="history-list" ref={listRef} role="listbox" aria-label="Chats">
          {empty ? (
            <div className="history-empty">
              {query.trim()
                ? `Nothing matches “${query.trim()}”.`
                : scope === "project"
                  ? "No chats in this project yet."
                  : "No chats yet."}
            </div>
          ) : (
            <>
              {sections.needs.length > 0 && (
                <div
                  className="history-head needs"
                  title="Loose ends: pick each one back up, or open it and mark it finished"
                >
                  <span>Needs you</span>
                  <span className="history-count">{sections.needs.length}</span>
                </div>
              )}
              {sections.needs.map((n) => row(n.row, n.label))}
              {sections.open.length > 0 && (
                <div className="history-head">
                  <span>{sections.needs.length > 0 ? "Other chats" : "Chats"}</span>
                </div>
              )}
              {sections.open.map((s) => row(s))}
              {sections.finished.length > 0 && (
                <div className="history-head finished" title="Marked finished — nothing owed here">
                  <span>Finished</span>
                  <span className="history-count">{sections.finished.length}</span>
                </div>
              )}
              {sections.finished.map((s) => row(s, undefined, true))}
            </>
          )}
        </div>
        <div className="history-hint">
          <kbd>↑</kbd>
          <kbd>↓</kbd> move · <kbd>↩</kbd> open · <kbd>esc</kbd> close
        </div>
      </div>

      {pendingDelete && (
        <DeleteChatModal chat={pendingDelete} onClose={() => setPendingDelete(null)} />
      )}
    </Modal>
  );
}

/** One chat: its standing as a dot, the title, why it needs you (if it does),
 *  then when · model · project · dev server. A chat already open as a tab
 *  says so; the delete icon shows on hover. */
function HistoryRow({
  row,
  status,
  need,
  finished,
  current,
  highlighted,
  asTab,
  project,
  onOpen,
  onHover,
  onDelete,
}: {
  row: SessionSummary;
  status: TabStatus;
  need?: string;
  finished: boolean;
  current: boolean;
  highlighted: boolean;
  asTab: boolean;
  /** The project's name, when the list spans every project. */
  project: string | null;
  onOpen: () => void;
  onHover: () => void;
  onDelete: () => void;
}) {
  const title = row.title?.trim() || "New chat";
  const model = shortModel(row.model);
  const when = row.created_at ? relativeTime(row.created_at) : "Not started yet";
  const previewPort = useStore((s) => {
    const p = s.previews[row.id];
    return p && p.phase === "ready" ? p.port : null;
  });
  const classes = [
    "history-item",
    status.state,
    current ? "active" : "",
    highlighted ? "highlighted" : "",
    need ? "needy" : "",
    finished ? "finished" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={classes} onMouseMove={onHover}>
      <button
        className="history-open"
        role="option"
        aria-selected={highlighted}
        tabIndex={-1}
        onClick={onOpen}
      >
        <StatusDot state={status.state} label={status.label} />
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
            {project && (
              <>
                <span className="history-sep">·</span>
                <span className="history-project" title={row.workspace}>
                  {project}
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
        {asTab && (
          <span className="history-tab-mark" title="Open as a tab">
            tab
          </span>
        )}
      </button>
      <button
        className="history-delete"
        title="Delete chat"
        aria-label={`Delete chat: ${title}`}
        tabIndex={-1}
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
