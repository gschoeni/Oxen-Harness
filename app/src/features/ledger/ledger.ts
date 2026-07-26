// The Ledger's domain logic, pure and component-free: fold the backend
// snapshot (plus live run status and per-workspace git) into the board the
// page renders. Everything here is *derived* — the only stored inputs are the
// transcript-level facts the snapshot carries and the settle marks; nothing in
// this module invents state that could drift from the truth.

import type { GitOverview, LedgerEntry, Project } from "../../lib/types";
import { basename } from "../../lib/format";

/** Where a thread stands, in trail language. Exactly one applies:
 *  - `running`  — work is in flight right now;
 *  - `dangling` — the transcript stops mid-turn and nothing is running:
 *                 the reply never arrived (error, out of credits, app quit);
 *  - `camp`     — idle with the last word spoken; waiting on the user;
 *  - `settled`  — tied off. */
export type ThreadState = "running" | "dangling" | "camp" | "settled";

/** Why an open thread earns a spot on the attention rail, strongest first. */
export type Need = "dangling" | "finished" | "plan-open" | "going-cold";

/** Idle thresholds, in days: visual weathering begins, the "going cold"
 *  warning, and the quiet slide into the archive. A thread is never deleted —
 *  the archive is a place, not a grave. */
export const WEATHER_DAYS = 3;
export const COLD_DAYS = 7;
export const ARCHIVE_DAYS = 14;

/** How many threads a train shows on the home board. Past this, the project
 *  deserves its own page — the board is for focus, not completeness. */
export const TRAIN_LIMIT = 3;

/** The shipping stretch: code leaves the machine, gets its review, lands.
 *  These stages live on the charted trail as ordinary waypoints (exactly
 *  named: pushed, pr-reviewed, merged) — the AGENT verifies them with its
 *  git/gh tools and marks them done; the board only renders and gates. */
export type ShipStage = "pushed" | "reviewed" | "merged";

/** Which shipping stage a waypoint name spells, if any. Tolerant of the
 *  dialects models actually write ("push", "pr reviewed", "code merged") —
 *  but on whole words only: a working waypoint like "code review", "review
 *  approach", or "fix merge conflicts" must never read as a shipping gate
 *  (substring matching once turned "approach" into a PR and "unmerged" into
 *  merged). The past-tense token is the signal that a stage is a gate. */
export function shipStage(name: string): ShipStage | null {
  const n = name.trim().toLowerCase().replace(/[-_]/g, " ").replace(/\s+/g, " ");
  const words = n.split(" ");
  if (words.includes("pushed") || n === "push") return "pushed";
  if (
    (words.includes("reviewed") &&
      (words.length === 1 || words.includes("pr") || words.includes("pull") || words.includes("code"))) ||
    n === "pr review"
  ) {
    return "reviewed";
  }
  if (words.includes("merged") || n === "merge") return "merged";
  return null;
}

export interface Thread {
  entry: LedgerEntry;
  state: ThreadState;
  /** Shipping waypoints the agent charted but hasn't verified done — the
   *  open loops that gate tie-off. Empty when clear (or never charted:
   *  the board refuses to block on stages nobody promised). */
  shipGates: ShipStage[];
  /** Running, but the agent is parked on a permission approval — the ride
   *  continues the moment the user joins and answers. The board paints this
   *  in the warning color with a join CTA: it outranks everything, because
   *  the agent is burning wall-clock waiting. */
  stuck: boolean;
  /** ✦ — activity landed after the board was last seen, and it isn't still
   *  running: something finished (or died) while the user wasn't looking. */
  fresh: boolean;
  /** Days since the last message, fractional. */
  idleDays: number;
  /** Weathering step for the visuals: 0 fresh, 1 aging, 2 weathered. */
  weather: 0 | 1 | 2;
  /** The strongest reason this thread needs the user, if any. */
  need: Need | null;
}

/** One wagon train: a workspace and its open threads travelling together. */
export interface Train {
  workspace: string;
  name: string;
  /** Every open thread, newest activity first (the project page shows all). */
  threads: Thread[];
  /** The TRAIN_LIMIT threads the home board shows: anything that needs the
   *  user or is running always makes the cut, then the newest fill in —
   *  presented in recency order so the train still reads chronologically. */
  visible: Thread[];
  /** How many open threads the home board is NOT showing. */
  hiddenCount: number;
  lastActivityAt: number;
  project: Project | null;
  git: GitOverview | null;
}

/** A project with nothing on the trail — collapsed to one quiet line. */
export interface QuietTrain {
  workspace: string;
  name: string;
  project: Project;
  /** Whether any archived (not settled) thread could be resurrected here. */
  lostCount: number;
}

export interface Board {
  /** Workspaces with open threads, newest activity first. */
  trains: Train[];
  /** Projects with no open threads. */
  quiet: QuietTrain[];
  /** Settled threads, newest settle first. */
  settled: Thread[];
  settledToday: number;
  settledWeek: number;
  /** Open-but-cold threads that slid off the board (idle past ARCHIVE_DAYS).
   *  Browsable and resurrectable — the amnesty that keeps day one guilt-free. */
  lost: Thread[];
  /** Seconds since the board was last seen; 0 when this is the first look. */
  awaySeconds: number;
  /** How many ✦ threads changed while the user was away. */
  freshCount: number;
}

export interface BoardInputs {
  entries: LedgerEntry[];
  /** The union of the snapshot's authoritative running set and any live
   *  "running" statuses the event bridge has seen since. */
  running: Set<string>;
  /** Sessions parked on a pending permission approval right now. */
  waiting?: Set<string>;
  lastSeen: number;
  projects: Project[];
  git: Record<string, GitOverview>;
  /** Unix seconds "now" — passed in so the fold is pure and testable. */
  now: number;
}

const DAY = 86_400;

/** Fold the raw inputs into the board. */
export function deriveBoard(inputs: BoardInputs): Board {
  const { entries, running, lastSeen, projects, git, now } = inputs;
  const waiting = inputs.waiting ?? new Set<string>();

  const threads = entries.map((entry) => deriveThread(entry, running, waiting, lastSeen, now));

  const settled = threads
    .filter((t) => t.state === "settled")
    .sort((a, b) => (b.entry.settle?.settled_at ?? 0) - (a.entry.settle?.settled_at ?? 0));
  const open = threads.filter((t) => t.state !== "settled");
  // Running threads never archive, no matter how long the turn has run.
  const lost = open.filter((t) => t.idleDays >= ARCHIVE_DAYS && t.state !== "running");
  const onBoard = open.filter((t) => !lost.includes(t));

  const byWorkspace = new Map<string, Thread[]>();
  for (const thread of onBoard) {
    const train = byWorkspace.get(thread.entry.workspace) ?? [];
    train.push(thread);
    byWorkspace.set(thread.entry.workspace, train);
  }
  const projectsByPath = new Map(projects.map((p) => [p.path, p]));
  const trains: Train[] = [...byWorkspace.entries()]
    .map(([workspace, threads]) => {
      threads.sort((a, b) => b.entry.last_activity_at - a.entry.last_activity_at);
      const visible = visibleThreads(threads);
      return {
        workspace,
        name: projectsByPath.get(workspace)?.name ?? basename(workspace),
        threads,
        visible,
        hiddenCount: threads.length - visible.length,
        lastActivityAt: Math.max(...threads.map((t) => t.entry.last_activity_at)),
        project: projectsByPath.get(workspace) ?? null,
        git: git[workspace] ?? null,
      };
    })
    .sort((a, b) => b.lastActivityAt - a.lastActivityAt);

  const lostByWorkspace = new Map<string, number>();
  for (const thread of lost) {
    const key = thread.entry.workspace;
    lostByWorkspace.set(key, (lostByWorkspace.get(key) ?? 0) + 1);
  }
  const quiet: QuietTrain[] = projects
    .filter((p) => !byWorkspace.has(p.path))
    .map((project) => ({
      workspace: project.path,
      name: project.name,
      project,
      lostCount: lostByWorkspace.get(project.path) ?? 0,
    }))
    .sort((a, b) => (b.project.last_used_at ?? 0) - (a.project.last_used_at ?? 0));

  const startOfToday = now - localSecondsSinceMidnight(now);
  return {
    trains,
    quiet,
    settled,
    settledToday: settled.filter((t) => (t.entry.settle?.settled_at ?? 0) >= startOfToday).length,
    settledWeek: settled.filter((t) => (t.entry.settle?.settled_at ?? 0) >= now - 7 * DAY).length,
    lost: lost.sort((a, b) => b.entry.last_activity_at - a.entry.last_activity_at),
    awaySeconds: lastSeen > 0 ? Math.max(0, now - lastSeen) : 0,
    freshCount: onBoard.filter((t) => t.fresh).length,
  };
}

/** Pick which of a train's threads the home board shows. Running and needy
 *  threads are ALWAYS visible — a working agent's live readout and a loose
 *  end are the board's whole point, so the cap may stretch to fit them. Only
 *  the calm remainder is capped: the newest fill whatever room is left, and
 *  the chosen set keeps its recency order so the train still reads as a
 *  timeline. Input must already be newest-first. */
function visibleThreads(threads: Thread[]): Thread[] {
  if (threads.length <= TRAIN_LIMIT) return threads;
  const mustShow = new Set(
    threads.filter((t) => t.state === "running" || t.need !== null),
  );
  let room = Math.max(0, TRAIN_LIMIT - mustShow.size);
  return threads.filter((t) => {
    if (mustShow.has(t)) return true;
    if (room > 0) {
      room -= 1;
      return true;
    }
    return false;
  });
}

/** The thread's display title: the model's own name for the work when it
 *  charted one, else the first user message. */
export function threadTitle(thread: Thread): string {
  return thread.entry.trail?.title || thread.entry.title;
}

/** One session's thread, wherever it lives on the board — a train, the
 *  settled tally, or the archive. Null before its first user turn. */
export function findThread(board: Board, sessionId: string): Thread | null {
  for (const train of board.trains) {
    const hit = train.threads.find((t) => t.entry.id === sessionId);
    if (hit) return hit;
  }
  return (
    board.settled.find((t) => t.entry.id === sessionId) ??
    board.lost.find((t) => t.entry.id === sessionId) ??
    null
  );
}

/** The stage label a thread wears: the current waypoint's name; "done" once
 *  every waypoint is passed; null when no trail was charted. */
export function currentStage(thread: Thread): string | null {
  const trail = thread.entry.trail;
  if (!trail || trail.waypoints.length === 0) return null;
  const current = trail.waypoints.find((w) => w.status === "current");
  if (current) return current.name;
  return trail.waypoints.every((w) => w.status === "done") ? "done" : null;
}

function deriveThread(
  entry: LedgerEntry,
  running: Set<string>,
  waiting: Set<string>,
  lastSeen: number,
  now: number,
): Thread {
  const isRunning = running.has(entry.id);
  const state: ThreadState = entry.settle
    ? "settled"
    : isRunning
      ? "running"
      : entry.mid_turn
        ? "dangling"
        : "camp";
  const idleDays = Math.max(0, now - entry.last_activity_at) / DAY;
  const fresh = !isRunning && lastSeen > 0 && entry.last_activity_at > lastSeen;
  return {
    entry,
    state,
    shipGates: (entry.trail?.waypoints ?? [])
      .filter((w) => w.status !== "done")
      .map((w) => shipStage(w.name))
      .filter((stage): stage is ShipStage => stage !== null),
    stuck: isRunning && waiting.has(entry.id),
    fresh,
    idleDays,
    weather: idleDays >= COLD_DAYS ? 2 : idleDays >= WEATHER_DAYS ? 1 : 0,
    need: threadNeed(state, entry, fresh, idleDays),
  };
}

/** The strongest claim an open thread has on the user's attention. A running
 *  thread never needs anyone — that's the whole point of agents. */
function threadNeed(
  state: ThreadState,
  entry: LedgerEntry,
  fresh: boolean,
  idleDays: number,
): Need | null {
  if (state === "running" || state === "settled") return null;
  if (state === "dangling") return "dangling";
  if (fresh) return "finished";
  if (entry.plan && entry.plan.done < entry.plan.total) return "plan-open";
  if (idleDays >= COLD_DAYS) return "going-cold";
  return null;
}

/** Seconds since local midnight for unix time `now` — "settled today" means
 *  the user's today, not UTC's. */
function localSecondsSinceMidnight(now: number): number {
  const d = new Date(now * 1000);
  return d.getHours() * 3600 + d.getMinutes() * 60 + d.getSeconds();
}

// ---- trail geometry ---------------------------------------------------------

/** Where the line's landmarks sit, as fractions of its length. The wagon
 *  travels start → camp while the work happens; camp → the settle ring is the
 *  human's stretch of trail (review it, push it, tie it off). */
export const CAMP_AT = 0.72;

/** A named station on the line — a waypoint the model charted. Shipping
 *  waypoints (`ship`) live on the camp → ring stretch and draw differently. */
export interface TrailStation {
  at: number;
  name: string;
  done: boolean;
  status: "done" | "current" | "ahead";
  ship: boolean;
}

export interface TrailShape {
  /** Named waypoint stations, when the model charted a trail. Working
   *  stations own the start → camp stretch (the last one sits exactly at
   *  camp, so finishing the working route IS arriving); shipping-named
   *  waypoints (pushed, pr-reviewed, merged) continue onto the camp → ring
   *  stretch — the loops that outlive the code. Positions are always
   *  monotonic in route order. */
  stations: TrailStation[];
  /** Whether the camp circle itself still needs drawing: true when no station
   *  occupies the camp position (no charted trail, or a ship-only route that
   *  starts from camp). When a working route's last station IS camp, drawing
   *  both would stack two landmarks on one spot. */
  camp: boolean;
  /** Plan tick positions along the working stretch. Only drawn when there are
   *  no stations — with both, the ticks would read as noise between them
   *  (plan progress still shows in the status line and waystation). */
  ticks: number[];
  ticksDone: number;
  /** 0..1 — the wagon's position on the line. */
  progress: number;
}

/** The trail geometry for one thread. Honest positioning: with a charted
 *  trail the wagon stands at its current station; with only a plan it sits at
 *  measured progress; with neither, mid-trail while working and at camp once
 *  the last word was spoken. */
export function trailShape(thread: Thread): TrailShape {
  const { entry, state } = thread;
  const waypoints = entry.trail?.waypoints ?? [];
  const plan = entry.plan;

  if (waypoints.length > 0) {
    const ship = waypoints.map((w) => shipStage(w.name) !== null);
    const workingCount = ship.filter((s) => !s).length;
    const shipCount = waypoints.length - workingCount;
    // The two-stretch layout (working stations up to camp, shipping stations
    // beyond it) assumes the route ships at the end — the standard shape the
    // trail tool teaches. A degenerate interleaved route (a working waypoint
    // after a shipping one) would make those independent ordinals walk the
    // wagon backwards, so it falls back to even spacing: positions must stay
    // monotonic in route order, whatever the model charted.
    const interleaved = ship.slice(ship.indexOf(true) + 1).includes(false) && ship.includes(true);
    let wi = 0;
    let si = 0;
    const stations = waypoints.map((w, i) => {
      const at = interleaved
        ? (i + 1) / (waypoints.length + 1)
        : ship[i]
          ? CAMP_AT + (++si / (shipCount + 1)) * (1 - CAMP_AT)
          : (++wi / Math.max(workingCount, 1)) * CAMP_AT;
      return { at, name: w.name, done: w.status === "done", status: w.status, ship: ship[i] };
    });
    // A ship-only route (a reopened thread closing its loops) starts from
    // camp: the working stretch is already travelled, and the camp circle
    // still needs drawing because no station sits there.
    const shipOnly = !interleaved && workingCount === 0;
    const base = state === "settled" ? 1 : stationProgress(waypoints, stations);
    const progress = shipOnly ? Math.max(base, CAMP_AT) : base;
    return { stations, camp: shipOnly || interleaved, ticks: [], ticksDone: 0, progress };
  }

  const ticks = plan
    ? Array.from({ length: plan.total }, (_, i) => ((i + 1) / (plan.total + 1)) * CAMP_AT)
    : [];

  if (state === "settled")
    return { stations: [], camp: true, ticks, ticksDone: plan?.done ?? 0, progress: 1 };

  let progress: number;
  if (plan && plan.done < plan.total) {
    // Between the last completed tick and the next: visibly "between stations".
    progress = ((plan.done + 0.5) / (plan.total + 1)) * CAMP_AT;
  } else if (state === "camp") {
    progress = CAMP_AT;
  } else {
    // Working (or dangling) without a plan: mid-stretch, honestly vague.
    progress = CAMP_AT * 0.55;
  }
  return { stations: [], camp: true, ticks, ticksDone: plan?.done ?? 0, progress };
}

/** Where the wagon stands on a charted route: at the current station; at the
 *  last done station when nothing is current; just past the trailhead when
 *  the route hasn't been ridden at all. The last station sits at camp, so a
 *  finished route means standing at camp — whatever the thread's state says. */
function stationProgress(waypoints: { status: string }[], stations: TrailStation[]): number {
  const current = waypoints.findIndex((w) => w.status === "current");
  if (current >= 0) return stations[current].at;
  let lastDone = -1;
  for (let i = 0; i < waypoints.length; i++) {
    if (waypoints[i].status === "done") lastDone = i;
  }
  return lastDone >= 0 ? stations[lastDone].at : 0.08;
}
