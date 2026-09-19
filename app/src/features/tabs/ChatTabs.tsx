// The tab strip above the chat: one tab per open chat in the current project.
//
// A tab is a bookmark into the store's multi-session world, not a process —
// the agent behind a closed tab keeps running, and every chat is a click away
// again through the history (the clock at the strip's end, ⌘K). Each tab
// wears its chat's standing as a dot (see tabStatus.ts), so one glance at the
// strip says what's running, what's waiting on you, and what finished while
// you looked elsewhere. The visible chat is the active tab: the store keeps
// that true on every session swap, so nothing here has to.
//
// Keyboard: see shortcuts.ts. Right-click: TabMenu.tsx.

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { History, Plus, X } from "lucide-react";
import { useStore } from "../../lib/store";
import type { Item } from "../chat/thread";
import { useThreads } from "../threads/useThreads";
import { DeleteChatModal } from "../history/DeleteChatModal";
import { StatusDot } from "./StatusDot";
import { TabMenu, type MenuAt } from "./TabMenu";
import { tabSignals, tabStatus, wantsUser, type TabStatus } from "./tabStatus";
import "./tabs.css";

export { useChatTabShortcuts } from "./shortcuts";

const NO_TABS: string[] = [];

/** The drag payload of a tab being reordered — its own type, so the chat's
 *  file-drop handlers (which look for files and workspace paths) ignore it. */
const TAB_MIME = "application/x-oxen-chat-tab";

/** How much of a first prompt a tab shows before the history's own clipped
 *  title takes over (matches the backend's TITLE_CHARS). */
const TITLE_CHARS = 200;

/** A tab as the strip paints it. `title` is the persisted one (the user's
 *  name for the chat, else its first message); null until the history has
 *  listed the chat, when the tab falls back to the prompt in its thread. */
interface TabView {
  id: string;
  title: string | null;
  status: TabStatus;
}

/** A tab mid-drag, and the slot it would land in: before `over`, or after
 *  it when the pointer is past its midpoint; `over` null means the end. */
interface Dragging {
  id: string;
  over: string | null;
  after: boolean;
}

export function ChatTabs() {
  const session = useStore((s) => s.session);
  const workspace = session?.workspace ?? null;
  const ids = useStore((s) => (workspace ? s.chatTabs[workspace] : undefined)) ?? NO_TABS;
  const sessions = useStore((s) => s.sessions);
  const runStatus = useStore((s) => s.runStatus);
  const approvals = useStore((s) => s.approvals);
  const question = useStore((s) => s.question);
  const resume = useStore((s) => s.resume);
  const newChat = useStore((s) => s.newChat);
  const closeTab = useStore((s) => s.closeTab);
  const closeOtherTabs = useStore((s) => s.closeOtherTabs);
  const closeTabsRight = useStore((s) => s.closeTabsRight);
  const moveTab = useStore((s) => s.moveTab);
  const renameSession = useStore((s) => s.renameSession);
  const setHistoryOpen = useStore((s) => s.setHistoryOpen);
  const addNotice = useStore((s) => s.addNotice);
  const threads = useThreads();

  const summaries = useMemo(() => new Map(sessions.map((s) => [s.id, s])), [sessions]);
  const statusOf = (id: string) =>
    tabStatus(tabSignals({ runStatus, approvals, question }, id, threads.get(id)));

  const tabs: TabView[] = ids.map((id) => ({
    id,
    title: summaries.get(id)?.title?.trim() || null,
    status: statusOf(id),
  }));

  // Chats in this project that need the user but have no tab: the history
  // button counts them, so closing a running tab never loses its outcome.
  const hiddenNeeds = workspace
    ? sessions.filter(
        (s) =>
          s.workspace === workspace &&
          s.source === "" &&
          !ids.includes(s.id) &&
          wantsUser(statusOf(s.id)),
      ).length
    : 0;

  // Keep the active tab in view as tabs open and the strip scrolls.
  const scrollRef = useRef<HTMLDivElement>(null);
  const currentId = session?.session_id;
  useEffect(() => {
    const el = scrollRef.current?.querySelector<HTMLElement>(".chat-tab.active");
    el?.scrollIntoView?.({ inline: "nearest", block: "nearest" });
  }, [currentId, ids.length]);
  const overflow = useOverflowEdges(scrollRef, ids.length);

  const [menu, setMenu] = useState<(MenuAt & { id: string }) | null>(null);
  /** The tab whose name is being edited in place (double-click, or Rename…). */
  const [editing, setEditing] = useState<string | null>(null);
  const [dragging, setDragging] = useState<Dragging | null>(null);
  const [pendingDelete, setPendingDelete] = useState<{ id: string; title: string | null } | null>(null);

  // Opening can fail when the chat was deleted behind our back (the CLI, a
  // purge from Settings). Say so; the tab stays until the user closes it or
  // the next boot's prune finds it gone.
  const open = (id: string) =>
    resume(id).catch((e: unknown) => addNotice(`Couldn't open that chat: ${String(e)}`));

  // ←/→ move focus along the strip (a roving tabindex); Home/End jump to the
  // ends. Enter or Space on the focused tab opens it (see Tab).
  function onStripKeyDown(e: KeyboardEvent) {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
    const els = Array.from(scrollRef.current?.querySelectorAll<HTMLElement>(".chat-tab") ?? []);
    if (!els.length) return;
    const at = els.indexOf(document.activeElement as HTMLElement);
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? els.length - 1
          : (at + (e.key === "ArrowRight" ? 1 : -1) + els.length) % els.length;
    e.preventDefault();
    els[next].focus();
  }

  /** Land the dragged tab where the indicator says. */
  function drop() {
    if (!dragging) return;
    const { id, over, after } = dragging;
    setDragging(null);
    const before = over === null ? null : after ? (ids[ids.indexOf(over) + 1] ?? null) : over;
    moveTab(id, before);
  }

  if (!session) return null;

  return (
    <div className="chat-tabs">
      <div
        className="chat-tabs-scroll"
        ref={scrollRef}
        role="tablist"
        aria-label="Open chats"
        data-overflow={overflow}
        onKeyDown={onStripKeyDown}
        // A mouse wheel has no sideways axis; let its vertical scroll walk
        // the strip. Trackpads already scroll it sideways natively.
        onWheel={(e) => {
          if (e.deltaX !== 0 || e.deltaY === 0) return;
          const el = e.currentTarget;
          if (el.scrollWidth <= el.clientWidth) return;
          el.scrollLeft += e.deltaY;
        }}
        // Dropping on the strip's empty tail sends the tab to the end.
        onDragOver={(e) => {
          if (!isTabDrag(e)) return;
          e.preventDefault();
          if (e.target === e.currentTarget && dragging && dragging.over !== null) {
            setDragging({ ...dragging, over: null, after: false });
          }
        }}
        onDrop={(e) => {
          if (!isTabDrag(e)) return;
          e.preventDefault();
          e.stopPropagation();
          drop();
        }}
      >
        {tabs.map((tab) => (
          <Tab
            key={tab.id}
            tab={tab}
            active={tab.id === currentId}
            editing={tab.id === editing}
            dragging={dragging?.id === tab.id}
            dropAt={dragging?.over === tab.id ? (dragging.after ? "after" : "before") : null}
            onOpen={() => void open(tab.id)}
            onClose={() => void closeTab(tab.id)}
            onMenu={(e) => setMenu({ id: tab.id, x: e.clientX, y: e.clientY })}
            onEdit={() => setEditing(tab.id)}
            onRename={(name) => {
              setEditing(null);
              if (name !== null) void renameSession(tab.id, name);
            }}
            onDragStart={(e) => {
              e.dataTransfer.setData(TAB_MIME, tab.id);
              e.dataTransfer.effectAllowed = "move";
              setDragging({ id: tab.id, over: null, after: false });
            }}
            onDragOver={(e) => {
              if (!isTabDrag(e) || !dragging || dragging.id === tab.id) return;
              e.preventDefault();
              e.stopPropagation();
              e.dataTransfer.dropEffect = "move";
              const rect = e.currentTarget.getBoundingClientRect();
              const after = e.clientX > rect.left + rect.width / 2;
              if (dragging.over !== tab.id || dragging.after !== after) {
                setDragging({ ...dragging, over: tab.id, after });
              }
            }}
            onDrop={(e) => {
              if (!isTabDrag(e)) return;
              e.preventDefault();
              e.stopPropagation();
              drop();
            }}
            onDragEnd={() => setDragging(null)}
          />
        ))}
      </div>
      {/* Right after the last tab, where a new one will appear — and never
          scrolled away with the tabs, since it sits outside the scroller. */}
      <button
        className="chat-tabs-btn chat-tabs-new"
        onClick={() => void newChat()}
        title="New chat (⌘T)"
        aria-label="New chat"
      >
        <Plus size={16} />
      </button>
      <div className="chat-tabs-actions">
        <button
          className="chat-tabs-btn"
          onClick={() => setHistoryOpen(true)}
          title="All chats (⌘K)"
          aria-label="All chats"
        >
          <History size={15} />
          {hiddenNeeds > 0 && (
            <span className="chat-tabs-badge" aria-label={`${hiddenNeeds} closed chats need you`}>
              {hiddenNeeds}
            </span>
          )}
        </button>
      </div>

      {menu && (
        <TabMenu
          at={menu}
          onClose={() => setMenu(null)}
          onPick={(action) => {
            const id = menu.id;
            setMenu(null);
            switch (action) {
              case "rename":
                setEditing(id);
                break;
              case "close":
                void closeTab(id);
                break;
              case "close-others":
                void closeOtherTabs(id);
                break;
              case "close-right":
                void closeTabsRight(id);
                break;
              case "copy-id":
                navigator.clipboard
                  ?.writeText(id)
                  .then(() => addNotice("Session id copied"))
                  .catch(() => addNotice("Couldn't copy the session id"));
                break;
              case "delete":
                setPendingDelete({ id, title: summaries.get(id)?.title ?? null });
                break;
            }
          }}
        />
      )}
      {pendingDelete && (
        <DeleteChatModal chat={pendingDelete} onClose={() => setPendingDelete(null)} />
      )}
    </div>
  );
}

const isTabDrag = (e: DragEvent) => Array.from(e.dataTransfer.types).includes(TAB_MIME);

/** Which edges of a horizontal scroller have content hidden past them, as a
 *  space-separated token list for a data attribute ("left", "right", both,
 *  or ""). Re-measured on scroll, on resize, and when `count` changes. */
function useOverflowEdges(ref: React.RefObject<HTMLElement | null>, count: number): string {
  const [edges, setEdges] = useState("");
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // A few pixels of slack: the scroller's own inner padding must not read
    // as hidden content and fade the last tab's close button.
    const SLACK = 6;
    const measure = () => {
      const left = el.scrollLeft > SLACK;
      const right = el.scrollLeft + el.clientWidth < el.scrollWidth - SLACK;
      setEdges([left && "left", right && "right"].filter(Boolean).join(" "));
    };
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    window.addEventListener("resize", measure);
    // The strip's own width changes with the docks; jsdom has no observer.
    const observer =
      typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(el);
    return () => {
      el.removeEventListener("scroll", measure);
      window.removeEventListener("resize", measure);
      observer?.disconnect();
    };
  }, [ref, count]);
  return edges;
}

/** The first line of a thread's first prompt — a chat's name from the moment
 *  it is sent, before the history has listed (and clipped) it itself. */
export function firstPrompt(items: Item[] | undefined): string | null {
  const first = items?.find((item) => item.kind === "user");
  const line = first?.text.trim().split("\n")[0]?.trim() ?? "";
  return line ? line.slice(0, TITLE_CHARS) : null;
}

function Tab({
  tab,
  active,
  editing,
  dragging,
  dropAt,
  onOpen,
  onClose,
  onMenu,
  onEdit,
  onRename,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
}: {
  tab: TabView;
  active: boolean;
  editing: boolean;
  /** This tab is the one being dragged. */
  dragging: boolean;
  /** The dragged tab would land beside this one, on this side. */
  dropAt: "before" | "after" | null;
  onOpen: () => void;
  onClose: () => void;
  onMenu: (e: MouseEvent) => void;
  onEdit: () => void;
  /** The edit ended: with the name to keep, or null to leave it as it was. */
  onRename: (name: string | null) => void;
  onDragStart: (e: DragEvent) => void;
  onDragOver: (e: DragEvent) => void;
  onDrop: (e: DragEvent) => void;
  onDragEnd: () => void;
}) {
  const { status } = tab;
  // Subscribed per tab as a string, so a streaming thread only re-renders
  // this tab when its first prompt actually changes (once).
  const prompt = useStore((s) => firstPrompt(s.threads[tab.id]));
  const title = tab.title ?? prompt ?? "New chat";
  const classes = [
    "chat-tab",
    status.state,
    active ? "active" : "",
    editing ? "editing" : "",
    dragging ? "dragging" : "",
    dropAt ? `drop-${dropAt}` : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      role="tab"
      tabIndex={active ? 0 : -1}
      aria-selected={active}
      className={classes}
      title={editing ? undefined : status.label ? `${title} — ${status.label}` : title}
      draggable={!editing}
      onClick={editing ? undefined : onOpen}
      onDoubleClick={editing ? undefined : onEdit}
      // Middle-click closes, as every tabbed surface does.
      onAuxClick={(e) => {
        if (e.button === 1) onClose();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e);
      }}
      onKeyDown={(e: KeyboardEvent) => {
        if (editing) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDrop={onDrop}
      onDragEnd={onDragEnd}
    >
      <StatusDot state={status.state} label={status.label} />
      {editing ? (
        <TabNameInput initial={title} onDone={onRename} />
      ) : (
        <span className="chat-tab-title">{title}</span>
      )}
      <button
        className="chat-tab-close"
        aria-label={`Close tab: ${title}`}
        title="Close (⌘W)"
        tabIndex={-1}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        <X size={13} />
      </button>
    </div>
  );
}

/** The in-place name editor. Enter keeps, Escape leaves it, and clicking
 *  away keeps too — a half-typed name is still the name they typed. A blank
 *  name hands the title back to the chat's first message. */
function TabNameInput({
  initial,
  onDone,
}: {
  initial: string;
  onDone: (name: string | null) => void;
}) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  // Enter (or Escape) unmounts the field, which blurs it: report once.
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (name: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(name);
  };
  const keep = () => finish(value.trim() === initial.trim() ? null : value);
  return (
    <input
      ref={ref}
      className="chat-tab-input"
      aria-label="Chat name"
      value={value}
      spellCheck={false}
      onChange={(e) => setValue(e.target.value)}
      onBlur={keep}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          keep();
        } else if (e.key === "Escape") {
          e.preventDefault();
          finish(null);
        }
      }}
    />
  );
}
