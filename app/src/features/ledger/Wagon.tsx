// One thread as a ledger row: the wagon line (title / trail / status) and its
// waystation — the unfolded entry with the thread's last word, its charted
// route, curation for the training dataset, the quiet delete, and the tie-off
// ritual. Shared by the home board's trains and a project page's full trail.

import { useEffect, useRef, useState } from "react";
import { CircleDot, MessageSquare, Trash2 } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import { currentStage, threadTitle, type Thread } from "./ledger";
import { Trail } from "./Trail";

export function WagonRow({
  thread,
  expanded,
  onToggle,
}: {
  thread: Thread;
  expanded: boolean;
  onToggle: () => void;
}) {
  // The tie-off ritual in flight: the trail pulls taut and the knot pops in
  // *before* the settle write lands, so closure is felt, not just recorded.
  const [settling, setSettling] = useState(false);
  const dust = useDust(thread);
  return (
    <div className={`ledger-wagon-block ${expanded ? "expanded" : ""}`}>
      <button
        className={`ledger-wagon weather-${thread.weather} ${thread.state}`}
        onClick={onToggle}
        aria-expanded={expanded}
        title={wagonTooltip(thread)}
      >
        <span className="ledger-wagon-title">
          {thread.fresh && <span className="ledger-fresh">✦</span>}
          {threadTitle(thread)}
        </span>
        <Trail thread={thread} settling={settling} dust={dust} />
        <span className="ledger-wagon-status">
          <TrainingFlag status={thread.entry.review_status} />
          {settling ? "tying off…" : statusLine(thread)}
        </span>
      </button>
      {thread.state === "running" && <LiveLine thread={thread} />}
      {expanded && (
        <Waystation thread={thread} settling={settling} onSettling={setSettling} />
      )}
    </div>
  );
}

/** The live readout under a riding wagon: the tool it's swinging right now,
 *  flashing as each call lands, honestly "thinking" between calls. Fed for
 *  every session — background chats report just as loudly as the one on
 *  screen. Idle rows never grow this line. */
function LiveLine({ thread }: { thread: Thread }) {
  const activity = useStore((s) => s.trailActivity[thread.entry.id]);
  return activity ? (
    // Keyed by `at`: every tool call remounts the readout and replays the
    // land-flash. The caret blinks while the agent thinks between calls.
    <div key={activity.at} className="ledger-wagon-live" aria-label="Live activity">
      ⚙ {activity.name}
      {activity.detail && <span className="ledger-wagon-live-detail"> {clipDetail(activity.detail)}</span>}
      <span className="ledger-live-caret" />
    </div>
  ) : (
    <div className="ledger-wagon-live thinking" aria-label="Live activity">
      thinking<span className="ledger-live-caret" />
    </div>
  );
}

/** One line of detail, tail-clipped — paths read best from the end. */
function clipDetail(detail: string): string {
  const flat = detail.replace(/\s+/g, " ").trim();
  return flat.length > 72 ? `…${flat.slice(-71)}` : flat;
}

/** A puff of dust behind a riding wagon, one per tool call. Motion is
 *  information here: dust means the agent is working its tools right now.
 *  Returns a changing key (0 = still); each change replays the puff. */
function useDust(thread: Thread): number {
  const count = useStore((s) => s.trailDust[thread.entry.id] ?? 0);
  const seen = useRef(count);
  const [puff, setPuff] = useState(0);
  useEffect(() => {
    if (count === seen.current) return;
    seen.current = count;
    if (thread.state === "running") setPuff((p) => p + 1);
  }, [count, thread.state]);
  return thread.state === "running" ? puff : 0;
}

/** How long the knot ritual plays before the settle write lands. Matches the
 *  trail's CSS transition; reduced-motion users get the write immediately. */
const KNOT_MS = 650;

/** The unfolded ledger entry: the story (the thread's last word, its charted
 *  route, the bookkeeping) with the quiet delete tucked in the top corner,
 *  and two ways out side by side beneath — ride back in, or tie it off.
 *  Training curation lives in the chat's Inspector drawer, not here. */
function Waystation({
  thread,
  settling,
  onSettling,
}: {
  thread: Thread;
  settling: boolean;
  onSettling: (v: boolean) => void;
}) {
  const open = useOpenThread();
  const settleThread = useStore((s) => s.settleThread);
  const { entry, state } = thread;
  const plan = entry.plan;
  const route = entry.trail?.waypoints ?? [];

  // One click, no ceremony: the knot ritual plays and the settle lands.
  // (Closing notes still exist on the wire for old settles; the UI just
  // stopped asking — tie-off should cost nothing.)
  function tieOff() {
    onSettling(true);
    const delay = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? 0 : KNOT_MS;
    window.setTimeout(() => {
      void settleThread(entry.id, "").finally(() => onSettling(false));
    }, delay);
  }

  return (
    <div className="ledger-waystation" role="region" aria-label={`${threadTitle(thread)} — waystation`}>
      <CutLoose thread={thread} disabled={settling} />
      <div className="ledger-waystation-story">
        {entry.last_reply ? (
          <blockquote className="ledger-waystation-reply">{clip(entry.last_reply, 240)}</blockquote>
        ) : (
          <p className="ledger-waystation-silence">
            {state === "running" ? "Still riding — no word yet." : "No parting word was recorded."}
          </p>
        )}
        {(route.length > 0 || (plan && plan.total > 0)) && (
          <div className="ledger-waystation-route">
            {route.map((wp, i) => (
              <span key={i} className={`route-${wp.status}`}>
                {i > 0 && " — "}
                {wp.name}
                {wp.status === "done" ? " ✓" : ""}
              </span>
            ))}
            {plan && plan.total > 0 && (
              <span className="ledger-waystation-plan">
                {route.length > 0 && " · "}
                plan {plan.done}/{plan.total}
                {plan.active ? ` · ${plan.active.toLowerCase()}` : plan.done >= plan.total ? " ✓" : ""}
              </span>
            )}
          </div>
        )}
        <div className="ledger-waystation-meta">
          {state === "dangling" && <span className="dangling">the reply never arrived</span>}
          <span>{entry.message_count} messages</span>
          <span>{relativeTime(entry.last_activity_at)}</span>
        </div>
      </div>
      <div className="ledger-waystation-ctas">
        <Button size="sm" onClick={() => open(thread)} disabled={settling}>
          <MessageSquare size={13} />
          {state === "dangling" ? "Pick it back up" : "Open chat"}
        </Button>
        {state !== "running" && (
          <Button variant="primary" size="sm" onClick={tieOff} disabled={settling}>
            <CircleDot size={13} />
            {settling ? "Tying…" : "Tie the knot"}
          </Button>
        )}
      </div>
    </div>
  );
}

/** The quiet way out: permanently delete the chat. Kept small and far from
 *  the primary actions; always behind a confirm. */
export function CutLoose({ thread, disabled }: { thread: Thread; disabled: boolean }) {
  const removeSession = useStore((s) => s.removeSession);
  const refreshLedger = useStore((s) => s.refreshLedger);
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);

  async function confirmDelete() {
    setDeleting(true);
    try {
      await removeSession(thread.entry.id);
      await refreshLedger();
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
  }

  return (
    <>
      <button
        className="ledger-cut-loose"
        title="Cut loose — delete this chat and its history"
        aria-label={`Delete chat: ${threadTitle(thread)}`}
        disabled={disabled}
        onClick={() => setConfirming(true)}
      >
        <Trash2 size={13} />
      </button>
      {confirming && (
        <Modal title="Cut this thread loose?" onClose={() => !deleting && setConfirming(false)}>
          <p className="delete-confirm-text">
            Permanently delete <strong>{threadTitle(thread)}</strong> and its chat history? There
            is no archive for this one — it’s gone for good.
          </p>
          <div className="delete-confirm-actions">
            <Button variant="ghost" onClick={() => setConfirming(false)} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="danger" onClick={confirmDelete} disabled={deleting}>
              {deleting ? "Cutting loose…" : "Delete chat"}
            </Button>
          </div>
        </Modal>
      )}
    </>
  );
}

function clip(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max).trimEnd()}…` : s;
}

/** Open a thread: resume its chat and ride out of the board. */
export function useOpenThread() {
  const resume = useStore((s) => s.resume);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  return (thread: Thread) => {
    void resume(thread.entry.id).then(() => setHomeOpen(false));
  };
}

/** A quiet mark that this chat has been curated for the training dataset:
 *  a solid flag when kept, a hollow one when rejected. */
function TrainingFlag({ status }: { status: string }) {
  if (status !== "kept" && status !== "rejected") return null;
  return (
    <span className={`ledger-training-flag ${status}`} title={`${status} for training data`}>
      {status === "kept" ? "⚑" : "⚐"}
    </span>
  );
}

function statusLine(thread: Thread): string {
  const plan = thread.entry.plan;
  const progress = plan && plan.total > 0 ? `${plan.done}/${plan.total}` : "";
  const when = relativeTime(thread.entry.last_activity_at);
  // The model's own word for where it is beats our generic verbs.
  const stage = currentStage(thread);
  switch (thread.state) {
    case "running":
      return [stage ?? "riding", progress].filter(Boolean).join(" · ");
    case "dangling":
      return `left dangling · ${when}`;
    case "camp":
      if (stage && stage !== "done") return `${stage} · ${when}`;
      if (plan && plan.done < plan.total) return `${progress} · ${when}`;
      return `done · ${when}`;
    case "settled":
      return `settled · ${relativeTime(thread.entry.settle?.settled_at ?? 0)}`;
  }
}

function wagonTooltip(thread: Thread): string {
  const active = thread.entry.plan?.active;
  if (thread.state === "running" && active) return active;
  if (thread.state === "dangling") return "The reply never arrived — open to pick it back up";
  return thread.entry.title;
}
