// Where every chat stands, pure and component-free: fold the backend
// snapshot (plus live run status and pending approvals) into one verdict per
// thread. Everything here is derived — the only stored inputs are the
// transcript-level facts the snapshot carries, the per-thread seen mark, and
// the one human-set boolean: finished.

import type { ApprovalRequestEvent, RunStatus, ThreadEntry, ThreadSnapshot } from "../../lib/types";

/** Exactly one applies:
 *  - `running`  — work is in flight right now;
 *  - `dangling` — the transcript stops mid-turn and nothing is running:
 *                 the reply never arrived (error, out of credits, app quit);
 *  - `idle`     — the last word was spoken; waiting on the user;
 *  - `finished` — the user marked it done. */
export type ThreadState = "running" | "dangling" | "idle" | "finished";

/** Why an open thread needs the user, strongest first. */
export type Need = "dangling" | "unseen";

export interface Thread {
  entry: ThreadEntry;
  state: ThreadState;
  /** Running, but the agent is parked on a permission approval — it
   *  continues the moment the user answers. Outranks everything, because the
   *  agent is burning wall-clock waiting. */
  stuck: boolean;
  /** Activity landed after the user last looked, and nothing is running:
   *  something finished (or died) while they weren't watching. */
  fresh: boolean;
  /** The strongest reason this thread needs the user, if any. */
  need: Need | null;
}

/** The store slices the verdicts are folded from. */
export interface ThreadSlices {
  threadsSnapshot: ThreadSnapshot | null;
  runStatus: Record<string, RunStatus | undefined>;
  approvals: Record<string, ApprovalRequestEvent | undefined>;
}

/** Every thread's verdict by session id: the snapshot's facts with the live
 *  run statuses and pending approvals layered on top. Empty until the first
 *  snapshot lands. The one assembly every reader shares. */
export function threadsFromState(s: ThreadSlices): Map<string, Thread> {
  const map = new Map<string, Thread>();
  if (!s.threadsSnapshot) return map;
  // The snapshot's running set survives restarts; live run statuses cover
  // turns started since it was taken. Union, never either alone.
  const running = new Set(s.threadsSnapshot.running);
  for (const [id, status] of Object.entries(s.runStatus)) {
    if (status === "running") running.add(id);
  }
  for (const entry of s.threadsSnapshot.entries) {
    map.set(entry.id, deriveThread(entry, running.has(entry.id), s.approvals[entry.id] !== undefined));
  }
  return map;
}

function deriveThread(entry: ThreadEntry, isRunning: boolean, waiting: boolean): Thread {
  const state: ThreadState =
    entry.finished_at > 0 ? "finished" : isRunning ? "running" : entry.mid_turn ? "dangling" : "idle";
  // Measured against the thread's own seen mark once it has one: leaving a
  // page can't quietly clear a loose end the user never looked at.
  const fresh = !isRunning && entry.seen_at > 0 && entry.last_activity_at > entry.seen_at;
  return {
    entry,
    state,
    // Stuck means running AND waiting: a stale approval on an idle chat isn't.
    stuck: isRunning && waiting,
    fresh,
    need: threadNeed(state, fresh),
  };
}

/** The strongest claim an open thread has on the user's attention. A running
 *  thread never needs anyone — that's the whole point of agents. */
function threadNeed(state: ThreadState, fresh: boolean): Need | null {
  if (state === "running" || state === "finished") return null;
  if (state === "dangling") return "dangling";
  if (fresh) return "unseen";
  return null;
}

/** Whether a thread has a claim on the user right now — the one predicate the
 *  project card's "N need you" pill and the chat list's "Needs you" section
 *  both count, so the number on the card is the number of rows inside. A
 *  stuck agent has `need === null` because it is running, yet it is the
 *  loudest claim there is. */
export function needsUser(thread: Thread): boolean {
  return thread.need !== null || thread.stuck;
}

/** Why the thread needs the user, in a few words. Null when nothing is owed. */
export function needLabel(thread: Thread): string | null {
  if (thread.stuck) return "waiting on your approval";
  switch (thread.need) {
    case "dangling":
      return "left dangling — reply never arrived";
    case "unseen":
      return "finished while you were away";
    case null:
      return null;
  }
}

/** Urgency order for needy threads, lowest first: a parked agent burns
 *  wall-clock, a dangler lost its reply, then a finish nobody looked at.
 *  Threads that need nothing sort last. */
export function needRank(thread: Thread): number {
  return thread.stuck ? 0 : rankOfNeed(thread.need);
}

/** [`needRank`] for a bare need, when there is no derived thread to ask. */
export function rankOfNeed(need: Need | null): number {
  switch (need) {
    case "dangling":
      return 1;
    case "unseen":
      return 2;
    case null:
      return 3;
  }
}

/** A workspace's open threads as a working set, most pressing first: the
 *  ones with a claim on the user in urgency order, then the rest newest
 *  first. Finished threads are left out — nothing is owed there. */
export function openThreadIds(threads: Map<string, Thread>, workspace: string): string[] {
  return [...threads.values()]
    .filter((t) => t.entry.workspace === workspace && t.state !== "finished")
    .sort((a, b) => needRank(a) - needRank(b) || b.entry.last_activity_at - a.entry.last_activity_at)
    .map((t) => t.entry.id);
}

/** A workspace's vital signs for its project card. */
export interface Vitals {
  /** Threads not marked finished. */
  open: number;
  running: number;
  /** Threads with a claim on the user — the same predicate the chat list
   *  sections on, so the pill's number is exactly the rows waiting inside. */
  needs: number;
}

export function workspaceVitals(threads: Map<string, Thread>, workspace: string): Vitals {
  const vitals: Vitals = { open: 0, running: 0, needs: 0 };
  for (const t of threads.values()) {
    if (t.entry.workspace !== workspace || t.state === "finished") continue;
    vitals.open += 1;
    if (t.state === "running") vitals.running += 1;
    if (needsUser(t)) vitals.needs += 1;
  }
  return vitals;
}
