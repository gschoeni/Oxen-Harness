// Home — the app's front door, with two lenses on the same board. The Ledger
// lens is the trail map: a wagon train per workspace with a trail line per
// thread (running rows carry their live tool readout right on the row), the
// settled tally, and the archive of threads lost to the trail. The cards
// lens is the calm project grid. Everything rendered is derived truth (see
// ledger.ts); the page's whole job is to make the state of many concurrent
// agents legible in one glance and every loose end one click from closed.

import { useEffect, useState } from "react";
import { FolderPlus, LayoutGrid, Plus, Rows3, Settings2, Trash2 } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
import { getUi, setUi } from "../../lib/uiState";
import { ProjectHome } from "../projects/ProjectHome";
import { RemoveProjectModal } from "../projects/RemoveProjectModal";
import { StartProjectModal } from "../projects/StartProjectModal";
import { threadTitle, type Board, type Train } from "./ledger";
import { ProjectCards } from "./ProjectCards";
import { useBoard } from "./useBoard";
import { CutLoose, SettledRow, useOpenThread, WagonRow } from "./Wagon";
import "./ledger.css";

export function HomePage() {
  const projects = useStore((s) => s.projects);
  const projectHomePath = useStore((s) => s.projectHomePath);
  const openProjectHome = useStore((s) => s.openProjectHome);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const refreshHistory = useStore((s) => s.refreshHistory);
  const refreshLedger = useStore((s) => s.refreshLedger);
  // Which project page is open is the STORE's decision (projectHomePath) —
  // the titlebar and other chrome navigate by setting it while Home is
  // already mounted, so a mount-time snapshot would leave their buttons dead.
  // The local state only mirrors it as a resolved Project object (and carries
  // in-page edits between refreshes).
  const [selected, setSelected] = useState<Project | null>(null);
  useEffect(() => {
    setSelected((current) =>
      projectHomePath
        ? current?.path === projectHomePath
          ? current
          : (projects.find((p) => p.path === projectHomePath) ?? null)
        : null,
    );
  }, [projectHomePath, projects]);
  const [starting, setStarting] = useState(false);

  // The board loads itself: on first mount (app start opens the Ledger without
  // going through setHomeOpen) and again whenever the window regains focus —
  // coming back to the app IS coming back to the trail, and the git banners
  // may have moved while other tools touched the repos.
  useEffect(() => {
    void refreshLedger();
    const onFocus = () => void refreshLedger();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshLedger]);

  const board = useBoard();

  async function projectChanged(project: Project) {
    setSelected((current) => ({
      ...project,
      session_count: current?.session_count ?? project.session_count,
      active: true,
    }));
    await refreshHistory();
  }

  return (
    <div className="home-overlay" role="dialog" aria-modal="true" aria-label="Home">
      {selected ? (
        <ProjectHome
          project={selected}
          // Back to the board through the store, so projectHomePath agrees
          // with what's on screen (and the board refreshes on return).
          onBack={() => setHomeOpen(true)}
          onProjectChanged={projectChanged}
        />
      ) : (
        <BoardView
          board={board}
          onOpenProject={(project) => openProjectHome(project.path)}
          onStartProject={() => setStarting(true)}
        />
      )}

      {starting && (
        <StartProjectModal
          onClose={() => setStarting(false)}
          onCreated={async (project) => {
            setStarting(false);
            await useStore.getState().enterProject(project.path);
          }}
        />
      )}
    </div>
  );
}

/** Home's two lenses: the project cards (altitude, calm view — the default)
 *  and the ledger (the trail map — threads, working view). Persisted — a
 *  lens is a habit. */
type HomeView = "ledger" | "cards";

function savedView(): HomeView {
  return getUi("homeView") === "ledger" ? "ledger" : "cards";
}

function BoardView({
  board,
  onOpenProject,
  onStartProject,
}: {
  board: Board | null;
  onOpenProject: (project: Project) => void;
  onStartProject: () => void;
}) {
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);

  // The one unfolded waystation, if any — opening another folds the first.
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [view, setView] = useState<HomeView>(savedView);

  function changeView(next: HomeView) {
    setView(next);
    setUi("homeView", next);
  }

  if (!board) return <main className="home-page" aria-busy="true" />;

  const bare = board.trains.length === 0 && board.quiet.length === 0 && board.lost.length === 0;
  return (
    <main className="home-page">
      <header className="home-header">
        <div>
          <div className="home-eyebrow">
            <span>{today()}</span>
            <AwayNote board={board} />
          </div>
          <h1 className="home-title">Home</h1>
        </div>
        <div className="home-header-actions">
          <div className="home-view-toggle" role="group" aria-label="Home view">
            <button
              className={`home-view-option ${view === "cards" ? "selected" : ""}`}
              aria-pressed={view === "cards"}
              title="Cards — one card per project"
              onClick={() => changeView("cards")}
            >
              <LayoutGrid size={13} /> cards
            </button>
            <button
              className={`home-view-option ${view === "ledger" ? "selected" : ""}`}
              aria-pressed={view === "ledger"}
              title="The Ledger — every thread on its trail line"
              onClick={() => changeView("ledger")}
            >
              <Rows3 size={13} /> ledger
            </button>
          </div>
          <Button variant="ghost" onClick={() => setSettingsOpen(true)}>
            <Settings2 size={16} /> Settings
          </Button>
          <Button variant="ghost" onClick={onStartProject}>
            <FolderPlus size={16} /> New project
          </Button>
        </div>
      </header>

      {bare ? (
        <button className="ledger-empty" onClick={onStartProject}>
          <span className="ledger-empty-icon">
            <FolderPlus size={24} />
          </span>
          <strong>Open your first trail</strong>
          <span>Choose a folder, describe the goal, and set your first agent riding.</span>
        </button>
      ) : view === "cards" ? (
        <ProjectCards board={board} onOpenProject={onOpenProject} />
      ) : (
        <>
          <section className="ledger-trains" aria-label="Open threads by project">
            {board.trains.map((train) => (
              <TrainSection
                key={train.workspace}
                train={train}
                onOpenProject={onOpenProject}
                expandedId={expandedId}
                onToggle={(id) => setExpandedId((cur) => (cur === id ? null : id))}
              />
            ))}
          </section>
          <QuietRow board={board} onOpenProject={onOpenProject} />
          <footer className="ledger-foot">
            <SettledLedger board={board} />
            <LostToTheTrail board={board} />
          </footer>
        </>
      )}
    </main>
  );
}

/** "away 14h · ✦ 2 while you were out" — the whole re-entry story, told by
 *  the board itself. Quiet returns (nothing new, or barely away) say nothing. */
function AwayNote({ board }: { board: Board }) {
  if (board.awaySeconds < 3600 && board.freshCount === 0) return null;
  const parts: string[] = [];
  if (board.awaySeconds >= 3600) parts.push(`away ${compactDuration(board.awaySeconds)}`);
  if (board.freshCount > 0)
    parts.push(`✦ ${board.freshCount} while you were out`);
  return <span className="home-away">{parts.join(" · ")}</span>;
}

function TrainSection({
  train,
  onOpenProject,
  expandedId,
  onToggle,
}: {
  train: Train;
  onOpenProject: (project: Project) => void;
  expandedId: string | null;
  onToggle: (id: string) => void;
}) {
  const enterProject = useStore((s) => s.enterProject);
  return (
    <section className="ledger-train" aria-label={train.name}>
      <header className="ledger-train-banner">
        {train.project ? (
          <button
            className="ledger-train-name"
            title={`${train.workspace} — open project`}
            onClick={() => onOpenProject(train.project as Project)}
          >
            {train.name}
          </button>
        ) : (
          <span className="ledger-train-name plain" title={train.workspace}>
            {train.name}
          </span>
        )}
        <GitChips train={train} />
        <span className="ledger-train-rule" />
        <button
          className="ledger-new-trail"
          title={`New thread in ${train.name}`}
          onClick={() => void enterProject(train.workspace)}
        >
          <Plus size={13} /> trail
        </button>
      </header>
      {/* The home board shows a few threads per train — focus over
          completeness. The project's own page carries the full trail; a
          workspace without a project has no page to click into, so it simply
          shows everything (scratch workspaces rarely grow trains anyway). */}
      {(train.project ? train.visible : train.threads).map((thread) => (
        <WagonRow
          key={thread.entry.id}
          thread={thread}
          expanded={expandedId === thread.entry.id}
          onToggle={() => onToggle(thread.entry.id)}
        />
      ))}
      {train.project && train.hiddenCount > 0 && (
        <button
          className="ledger-train-more"
          title={`See all ${train.threads.length} threads in ${train.name}`}
          onClick={() => onOpenProject(train.project as Project)}
        >
          …and {train.hiddenCount} more on this trail
        </button>
      )}
    </section>
  );
}

/** The train banner's git facts: branch, dirt, and divergence. Workspace
 *  truth, deliberately not attributed to any one thread. */
function GitChips({ train }: { train: Train }) {
  const git = train.git;
  if (!git) return null;
  return (
    <span className="ledger-git">
      <span className="ledger-git-branch">{git.branch}</span>
      {git.dirty_files > 0 && (
        <span className="ledger-git-chip" title={`${git.dirty_files} changed file${git.dirty_files === 1 ? "" : "s"}`}>
          ±{git.dirty_files}
        </span>
      )}
      {git.ahead > 0 && (
        <span className="ledger-git-chip ahead" title={`${git.ahead} commit${git.ahead === 1 ? "" : "s"} not pushed`}>
          ↑{git.ahead}
        </span>
      )}
      {git.behind > 0 && (
        <span className="ledger-git-chip" title={`${git.behind} commit${git.behind === 1 ? "" : "s"} behind upstream`}>
          ↓{git.behind}
        </span>
      )}
      {git.dirty_files === 0 && git.ahead === 0 && git.behind === 0 && (
        <span className="ledger-git-chip clean" title={git.has_upstream ? "Clean and pushed" : "Clean (no upstream)"}>
          clean
        </span>
      )}
    </span>
  );
}


function QuietRow({
  board,
  onOpenProject,
}: {
  board: Board;
  onOpenProject: (project: Project) => void;
}) {
  const removeProject = useStore((s) => s.removeProject);
  // The project queued for removal (drives the confirm modal).
  const [pendingDelete, setPendingDelete] = useState<Project | null>(null);
  if (board.quiet.length === 0) return null;

  return (
    <section className="ledger-quiet" aria-label="Quiet projects">
      {board.quiet.map((q) => (
        <div key={q.workspace} className="ledger-quiet-row">
          <button
            className="ledger-quiet-open"
            title={`${q.workspace} — open project`}
            onClick={() => onOpenProject(q.project)}
          >
            <span className="ledger-quiet-name">{q.name}</span>
            <span className="ledger-quiet-note">
              {q.lostCount > 0
                ? `quiet · ${q.lostCount} on the wind`
                : q.project.last_used_at
                  ? `quiet · ${relativeTime(q.project.last_used_at)}`
                  : "no rides yet"}
            </span>
          </button>
          <button
            className="ledger-quiet-delete"
            title="Remove project"
            aria-label={`Remove project: ${q.name}`}
            onClick={() => setPendingDelete(q.project)}
          >
            <Trash2 size={14} />
          </button>
        </div>
      ))}

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
    </section>
  );
}

function SettledLedger({ board }: { board: Board }) {
  const [openList, setOpenList] = useState(false);
  if (board.settled.length === 0) return null;
  return (
    <section className="ledger-settled" aria-label="Settled threads">
      <button className="ledger-foot-toggle" onClick={() => setOpenList((v) => !v)} aria-expanded={openList}>
        <h2 className="ledger-label">Settled</h2>
        <span className="ledger-foot-tally">
          {board.settledToday > 0 && <span>today {board.settledToday}</span>}
          <span>this week {board.settledWeek}</span>
          <span className="ledger-foot-count">({board.settled.length})</span>
        </span>
      </button>
      {openList && (
        <div className="ledger-foot-rows">
          {board.settled.slice(0, 30).map((thread) => (
            <SettledRow key={thread.entry.id} thread={thread} />
          ))}
          {board.settled.length > 30 && (
            <div className="ledger-foot-more">and {board.settled.length - 30} more, settled and gone</div>
          )}
        </div>
      )}
    </section>
  );
}

function LostToTheTrail({ board }: { board: Board }) {
  const [openList, setOpenList] = useState(false);
  const open = useOpenThread();
  const removeSessions = useStore((s) => s.removeSessions);
  // The purge: pick some (or all) of the archive and let it go for good.
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  if (board.lost.length === 0) return null;

  const allIds = board.lost.map((t) => t.entry.id);
  const allPicked = picked.size === allIds.length;

  function toggle(id: string) {
    setPicked((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function confirmDelete() {
    setDeleting(true);
    try {
      await removeSessions([...picked]);
      setPicked(new Set());
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
  }

  return (
    <section className="ledger-lost" aria-label="Lost to the trail">
      <button className="ledger-foot-toggle" onClick={() => setOpenList((v) => !v)} aria-expanded={openList}>
        <h2 className="ledger-label">Lost to the trail</h2>
        <span className="ledger-foot-tally">
          <span className="ledger-foot-count">({board.lost.length})</span>
        </span>
      </button>
      {openList && (
        <div className="ledger-foot-rows">
          <div className="ledger-lost-controls">
            <button
              className="ledger-row-action"
              onClick={() => setPicked(allPicked ? new Set() : new Set(allIds))}
            >
              {allPicked ? "select none" : "select all"}
            </button>
            {picked.size > 0 && (
              <button className="ledger-row-action ledger-lost-purge" onClick={() => setConfirming(true)}>
                <Trash2 size={12} /> delete {picked.size}
              </button>
            )}
          </div>
          {board.lost.slice(0, 40).map((thread) => (
            <div key={thread.entry.id} className="ledger-lost-row">
              <input
                type="checkbox"
                className="ledger-lost-pick"
                aria-label={`Select: ${threadTitle(thread)}`}
                checked={picked.has(thread.entry.id)}
                onChange={() => toggle(thread.entry.id)}
              />
              <span className="ledger-lost-title">{threadTitle(thread)}</span>
              <span className="ledger-lost-where">{thread.entry.workspace.split("/").pop()}</span>
              <span className="ledger-lost-when">{relativeTime(thread.entry.last_activity_at)}</span>
              <button
                className="ledger-row-action"
                title="Rekindle — reopen this thread"
                onClick={() => open(thread)}
              >
                rekindle
              </button>
              <CutLoose thread={thread} disabled={deleting} />
            </div>
          ))}
          {board.lost.length > 40 && (
            <div className="ledger-foot-more">
              and {board.lost.length - 40} older still{allPicked ? " (selected too)" : ""}
            </div>
          )}
        </div>
      )}

      {confirming && (
        <Modal title={`Delete ${picked.size} chat${picked.size === 1 ? "" : "s"}?`} onClose={() => !deleting && setConfirming(false)}>
          <p className="delete-confirm-text">
            Permanently delete <strong>{picked.size}</strong> archived chat
            {picked.size === 1 ? "" : "s"} and their history? There is no archive below this one —
            they’re gone for good.
          </p>
          <div className="delete-confirm-actions">
            <Button variant="ghost" onClick={() => setConfirming(false)} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="danger" onClick={confirmDelete} disabled={deleting}>
              {deleting ? "Deleting…" : `Delete ${picked.size}`}
            </Button>
          </div>
        </Modal>
      )}
    </section>
  );
}


function today(): string {
  return new Date().toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
}

function compactDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  if (hours < 1) return `${Math.max(1, Math.floor(seconds / 60))}m`;
  if (hours < 48) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}
