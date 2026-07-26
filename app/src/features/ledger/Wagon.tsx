// One thread as a ledger row: the wagon line (title / trail / status) and its
// waystation — the unfolded entry with the thread's last word, its charted
// route, curation for the training dataset, the quiet delete, and the tie-off
// ritual. Shared by the home board's trains and a project page's full trail.

import { useEffect, useRef, useState } from "react";
import { CircleDot, MessageSquare, Trash2 } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import { relativeTime, truncate } from "../../lib/format";
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
  const open = useOpenThread();
  return (
    <div className={`ledger-wagon-block ${expanded ? "expanded" : ""}`}>
      <button
        className={`ledger-wagon weather-${thread.weather} ${thread.state} ${thread.stuck ? "stuck" : ""}`}
        // A stuck agent has exactly one useful action — join and answer — so
        // its whole row is that door instead of unfolding a waystation.
        onClick={thread.stuck ? () => open(thread) : onToggle}
        aria-expanded={expanded}
        aria-label={wagonTooltip(thread)}
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
  if (thread.stuck) {
    return (
      <div className="ledger-wagon-live stuck" aria-label="Waiting for approval">
        ⏸ waiting for your approval — join the chat →
      </div>
    );
  }
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
  const { entry, state } = thread;
  const plan = entry.plan;
  const route = entry.trail?.waypoints ?? [];

  return (
    <div className="ledger-waystation" role="region" aria-label={`${threadTitle(thread)} — waystation`}>
      <CutLoose thread={thread} disabled={settling} />
      <div className="ledger-waystation-story">
        {entry.last_reply ? (
          <blockquote className="ledger-waystation-reply">{truncate(entry.last_reply, 240)}</blockquote>
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
          <ShipFacts thread={thread} />
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
          <TieKnot thread={thread} settling={settling} onSettling={onSettling} />
        )}
      </div>
    </div>
  );
}

/** The knot, gated by the shipping loops. Clear gates: one click, ritual,
 *  done. Unmet gates (unpushed work, an unreviewed or unmerged PR): the
 *  button turns hesitant and a confirm names every loop still open — closing
 *  eyes-open stays possible, closing by accident doesn't. */
export function TieKnot({
  thread,
  settling,
  onSettling,
}: {
  thread: Thread;
  settling: boolean;
  onSettling: (v: boolean) => void;
}) {
  const settleThread = useStore((s) => s.settleThread);
  const [confirming, setConfirming] = useState(false);
  const gates = thread.shipGates;

  function ritual() {
    setConfirming(false);
    onSettling(true);
    const delay = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? 0 : KNOT_MS;
    window.setTimeout(() => {
      void settleThread(thread.entry.id, "").finally(() => onSettling(false));
    }, delay);
  }

  if (gates.length === 0) {
    return (
      <Button variant="primary" size="sm" onClick={ritual} disabled={settling}>
        <CircleDot size={13} />
        {settling ? "Tying…" : "Tie the knot"}
      </Button>
    );
  }

  return (
    <>
      <Button
        variant="ghost"
        size="sm"
        className="ledger-tie-gated"
        title={`Still open: ${gates.map(gateLabel).join(", ")}`}
        onClick={() => setConfirming(true)}
        disabled={settling}
      >
        <CircleDot size={13} />
        {settling ? "Tying…" : "Tie off anyway…"}
      </Button>
      {confirming && (
        <Modal title="Loops still open" onClose={() => setConfirming(false)}>
          <p className="delete-confirm-text">
            This thread hasn’t finished shipping:
          </p>
          <ul className="ledger-gate-list">
            {gates.map((gate) => (
              <li key={gate}>{gateLabel(gate)}</li>
            ))}
          </ul>
          <p className="delete-confirm-text">
            Tie it off anyway? The knot won’t push, review, or merge anything for you.
          </p>
          <div className="delete-confirm-actions">
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Keep it open
            </Button>
            <Button variant="primary" onClick={ritual}>
              <CircleDot size={13} /> Tie off anyway
            </Button>
          </div>
        </Modal>
      )}
    </>
  );
}

function gateLabel(gate: "pushed" | "reviewed" | "merged"): string {
  switch (gate) {
    case "pushed":
      return "code not pushed";
    case "reviewed":
      return "PR not reviewed";
    case "merged":
      return "PR not merged";
  }
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

/** The open shipping loops, spelled out in warning — the closed ones are
 *  already visible as ✓ on the charted route above. */
function ShipFacts({ thread }: { thread: Thread }) {
  if (thread.shipGates.length === 0) return null;
  return (
    <>
      {thread.shipGates.map((gate) => (
        <span key={gate} className="ship-open">
          {gateLabel(gate)}
        </span>
      ))}
    </>
  );
}

/** One settled thread's ledger line: the check, the title, the closing note,
 *  when it was tied off — and the untie that brings it back to the trail.
 *  Shared by the home board's settled tally and a project page's own. */
export function SettledRow({ thread }: { thread: Thread }) {
  const reopenThread = useStore((s) => s.reopenThread);
  return (
    <div className="ledger-settled-row">
      <span className="ledger-settled-check">✓</span>
      <span className="ledger-settled-title">{threadTitle(thread)}</span>
      {thread.entry.settle?.note && (
        <span className="ledger-settled-note">“{thread.entry.settle.note}”</span>
      )}
      <span className="ledger-settled-when">
        {relativeTime(thread.entry.settle?.settled_at ?? 0)}
      </span>
      <button
        className="ledger-row-action"
        title="Untie — bring this thread back to the trail"
        onClick={() => void reopenThread(thread.entry.id)}
      >
        untie
      </button>
    </div>
  );
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

export function statusLine(thread: Thread): string {
  const plan = thread.entry.plan;
  const progress = plan && plan.total > 0 ? `${plan.done}/${plan.total}` : "";
  const when = relativeTime(thread.entry.last_activity_at);
  // The model's own word for where it is beats our generic verbs.
  const stage = currentStage(thread);
  switch (thread.state) {
    case "running":
      if (thread.stuck) return "waiting on you";
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
  if (thread.stuck) return "Waiting for your approval — click to join the chat";
  const active = thread.entry.plan?.active;
  if (thread.state === "running" && active) return active;
  if (thread.state === "dangling") return "The reply never arrived — open to pick it back up";
  return thread.entry.title;
}
