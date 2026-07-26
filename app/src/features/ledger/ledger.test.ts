import { describe, expect, it } from "vitest";
import type { LedgerEntry, Project } from "../../lib/types";
import {
  ARCHIVE_DAYS,
  shipStage,
  findThread,
  COLD_DAYS,
  currentStage,
  deriveBoard,
  threadTitle,
  TRAIN_LIMIT,
  trailShape,
  type BoardInputs,
} from "./ledger";

const NOW = 1_753_000_000;
const DAY = 86_400;

function entry(overrides: Partial<LedgerEntry> = {}): LedgerEntry {
  return {
    id: "s1",
    workspace: "/work/app",
    model: "m",
    created_at: NOW - DAY,
    last_activity_at: NOW - 3_600,
    title: "fix the flaky test",
    last_reply: "",
    message_count: 8,
    mid_turn: false,
    plan: null,
    trail: null,
    review_status: "",
    settle: null,
    ...overrides,
  };
}

function project(path: string, name = ""): Project {
  return {
    path,
    name: name || path.split("/").pop() || path,
    description: "",
    instructions: "",
    context: [],
    session_count: 1,
    active: false,
    last_used_at: NOW - DAY,
  };
}

function inputs(overrides: Partial<BoardInputs> = {}): BoardInputs {
  return {
    entries: [],
    running: new Set(),
    lastSeen: 0,
    projects: [],
    git: {},
    now: NOW,
    ...overrides,
  };
}

describe("thread states", () => {
  it("classifies running, dangling, camp, and settled", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "run", mid_turn: true }),
          entry({ id: "dangle", mid_turn: true }),
          entry({ id: "camp" }),
          entry({ id: "tied", settle: { settled_at: NOW - 60, note: "" } }),
        ],
        running: new Set(["run"]),
      }),
    );
    const states = Object.fromEntries(
      [...board.trains.flatMap((t) => t.threads), ...board.settled].map((t) => [
        t.entry.id,
        t.state,
      ]),
    );
    expect(states).toEqual({
      run: "running",
      dangle: "dangling",
      camp: "camp",
      tied: "settled",
    });
  });

  it("marks threads fresh only when they changed after last seen and are not running", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "new", last_activity_at: NOW - 60 }),
          entry({ id: "old", last_activity_at: NOW - 2 * DAY }),
          entry({ id: "busy", last_activity_at: NOW - 60 }),
        ],
        running: new Set(["busy"]),
        lastSeen: NOW - DAY,
      }),
    );
    const fresh = board.trains
      .flatMap((t) => t.threads)
      .filter((t) => t.fresh)
      .map((t) => t.entry.id);
    expect(fresh).toEqual(["new"]);
    expect(board.freshCount).toBe(1);
    expect(board.awaySeconds).toBe(DAY);
  });

  it("first visit has no freshness story at all", () => {
    const board = deriveBoard(
      inputs({ entries: [entry({ last_activity_at: NOW - 60 })], lastSeen: 0 }),
    );
    expect(board.freshCount).toBe(0);
    expect(board.awaySeconds).toBe(0);
  });
});

describe("thread needs", () => {
  it("names each thread's strongest claim on the user", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "cold", last_activity_at: NOW - (COLD_DAYS + 1) * DAY }),
          entry({
            id: "plan",
            last_activity_at: NOW - 2 * DAY,
            plan: { done: 2, total: 5, active: null },
          }),
          entry({ id: "finished", last_activity_at: NOW - 600 }),
          entry({ id: "dangle", mid_turn: true, last_activity_at: NOW - 2 * DAY }),
        ],
        lastSeen: NOW - DAY,
      }),
    );
    const needs = Object.fromEntries(
      board.trains.flatMap((t) => t.threads).map((t) => [t.entry.id, t.need]),
    );
    expect(needs).toEqual({
      dangle: "dangling",
      finished: "finished",
      plan: "plan-open",
      cold: "going-cold",
    });
  });

  it("running and content threads need nothing", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "busy", mid_turn: true }),
          entry({ id: "content", last_activity_at: NOW - 3_600 }),
        ],
        running: new Set(["busy"]),
      }),
    );
    expect(board.trains.flatMap((t) => t.threads).every((t) => t.need === null)).toBe(true);
  });
});

describe("trains and quiet projects", () => {
  it("groups open threads by workspace, newest train first, with names and git", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "a1", workspace: "/work/app", last_activity_at: NOW - 3_600 }),
          entry({ id: "a2", workspace: "/work/app", last_activity_at: NOW - 60 }),
          entry({ id: "b1", workspace: "/work/blog", last_activity_at: NOW - 7_200 }),
        ],
        projects: [project("/work/app", "App"), project("/work/blog")],
        git: { "/work/app": { branch: "main", dirty_files: 2, ahead: 1, behind: 0, has_upstream: true } },
      }),
    );
    expect(board.trains.map((t) => t.name)).toEqual(["App", "blog"]);
    expect(board.trains[0].threads.map((t) => t.entry.id)).toEqual(["a2", "a1"]);
    expect(board.trains[0].git?.dirty_files).toBe(2);
    expect(board.trains[1].git).toBeNull();
  });

  it("a workspace with threads but no project still gets a train, named by folder", () => {
    const board = deriveBoard(
      inputs({ entries: [entry({ workspace: "/tmp/scratch-pad" })] }),
    );
    expect(board.trains[0].name).toBe("scratch-pad");
    expect(board.trains[0].project).toBeNull();
  });

  it("projects without open threads collapse to quiet lines carrying their lost count", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({
            id: "ancient",
            workspace: "/work/dust",
            last_activity_at: NOW - (ARCHIVE_DAYS + 1) * DAY,
          }),
        ],
        projects: [project("/work/dust", "Dust")],
      }),
    );
    expect(board.trains).toEqual([]);
    expect(board.quiet.map((q) => q.name)).toEqual(["Dust"]);
    expect(board.quiet[0].lostCount).toBe(1);
  });
});

describe("amnesty and the archive", () => {
  it("threads idle past the archive line are lost, not shown, never deleted", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "alive", last_activity_at: NOW - DAY }),
          entry({ id: "cold", last_activity_at: NOW - (ARCHIVE_DAYS + 2) * DAY }),
        ],
      }),
    );
    expect(board.trains.flatMap((t) => t.threads).map((t) => t.entry.id)).toEqual(["alive"]);
    expect(board.lost.map((t) => t.entry.id)).toEqual(["cold"]);
  });

  it("a running thread never archives, no matter how old its last message is", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "marathon", last_activity_at: NOW - (ARCHIVE_DAYS + 5) * DAY, mid_turn: true }),
        ],
        running: new Set(["marathon"]),
      }),
    );
    expect(board.lost).toEqual([]);
    expect(board.trains[0].threads[0].state).toBe("running");
  });

  it("settled threads tally today and this week", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "today", settle: { settled_at: NOW - 60, note: "shipped" } }),
          entry({ id: "midweek", settle: { settled_at: NOW - 3 * DAY, note: "" } }),
          entry({ id: "lastMonth", settle: { settled_at: NOW - 20 * DAY, note: "" } }),
        ],
      }),
    );
    expect(board.settled.map((t) => t.entry.id)).toEqual(["today", "midweek", "lastMonth"]);
    expect(board.settledToday).toBe(1);
    expect(board.settledWeek).toBe(2);
  });
});

describe("stuck threads and finding one", () => {
  it("a running thread parked on an approval is stuck; the rest are not", () => {
    const board = deriveBoard(
      inputs({
        entries: [entry({ id: "parked", mid_turn: true }), entry({ id: "busy", mid_turn: true })],
        running: new Set(["parked", "busy"]),
        waiting: new Set(["parked"]),
      }),
    );
    const threads = board.trains[0].threads;
    expect(threads.find((t) => t.entry.id === "parked")?.stuck).toBe(true);
    expect(threads.find((t) => t.entry.id === "busy")?.stuck).toBe(false);
    // An idle session with a stale approval id is not stuck — stuck means
    // running AND waiting.
    const idle = deriveBoard(
      inputs({ entries: [entry({ id: "idle" })], waiting: new Set(["idle"]) }),
    );
    expect(idle.trains[0].threads[0].stuck).toBe(false);
  });

  it("findThread locates a session in trains, settled, archive — or nowhere", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "open" }),
          entry({ id: "tied", settle: { settled_at: NOW - 60, note: "" } }),
          entry({ id: "cold", last_activity_at: NOW - (ARCHIVE_DAYS + 1) * DAY }),
        ],
      }),
    );
    expect(findThread(board, "open")?.state).toBe("camp");
    expect(findThread(board, "tied")?.state).toBe("settled");
    expect(findThread(board, "cold")?.entry.id).toBe("cold");
    expect(findThread(board, "nope")).toBeNull();
  });
});

describe("the home board's train cap", () => {
  const at = (offset: number) => NOW - offset;

  it("shows at most TRAIN_LIMIT threads, never hiding one that needs the user", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "new1", last_activity_at: at(100) }),
          entry({ id: "new2", last_activity_at: at(200) }),
          entry({ id: "new3", last_activity_at: at(300) }),
          // Oldest of all, but dangling — it must still make the cut.
          entry({ id: "dangle", mid_turn: true, last_activity_at: at(5 * DAY) }),
        ],
      }),
    );
    const train = board.trains[0];
    expect(train.visible).toHaveLength(TRAIN_LIMIT);
    expect(train.hiddenCount).toBe(1);
    expect(train.visible.map((t) => t.entry.id)).toEqual(["new1", "new2", "dangle"]);
    // The full list is untouched for the project page.
    expect(train.threads).toHaveLength(4);
  });

  it("running and needy threads stretch the cap — a live readout is never hidden", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "fresh1", last_activity_at: at(50) }),
          entry({ id: "fresh2", last_activity_at: at(60) }),
          entry({ id: "run1", last_activity_at: at(100), mid_turn: true }),
          entry({ id: "run2", last_activity_at: at(2 * DAY), mid_turn: true }),
          entry({ id: "dangle", mid_turn: true, last_activity_at: at(3 * DAY) }),
          entry({ id: "calm", last_activity_at: at(4 * DAY) }),
        ],
        running: new Set(["run1", "run2"]),
        lastSeen: at(DAY),
      }),
    );
    const train = board.trains[0];
    const ids = train.visible.map((t) => t.entry.id);
    // Both running agents and the dangler ride no matter what; the fresh
    // (needy) threads too. Only the calm one is capped away.
    expect(ids).toContain("run1");
    expect(ids).toContain("run2");
    expect(ids).toContain("dangle");
    expect(ids).not.toContain("calm");
    expect(train.hiddenCount).toBe(train.threads.length - ids.length);
  });

  it("small trains show everything", () => {
    const board = deriveBoard(
      inputs({ entries: [entry({ id: "a" }), entry({ id: "b", last_activity_at: NOW - 999 })] }),
    );
    expect(board.trains[0].visible).toHaveLength(2);
    expect(board.trains[0].hiddenCount).toBe(0);
  });
});

describe("the charted trail", () => {
  const charted = entry({
    title: "please look into that flaky test thing when you can",
    trail: {
      title: "fix flaky sse retry test",
      waypoints: [
        { name: "define", status: "done" },
        { name: "implement", status: "current" },
        { name: "review", status: "ahead" },
      ],
    },
  });

  it("the model's own title supersedes the first user message", () => {
    const board = deriveBoard(inputs({ entries: [charted] }));
    const thread = board.trains[0].threads[0];
    expect(threadTitle(thread)).toBe("fix flaky sse retry test");
    expect(currentStage(thread)).toBe("implement");
  });

  it("a finished route reads done; no trail reads nothing", () => {
    const done = entry({
      trail: {
        title: "t",
        waypoints: [
          { name: "define", status: "done" },
          { name: "review", status: "done" },
        ],
      },
    });
    const board = deriveBoard(inputs({ entries: [done, entry({ id: "plain" })] }));
    const threads = board.trains[0].threads;
    expect(currentStage(threads.find((t) => t.entry.id === "s1")!)).toBe("done");
    expect(currentStage(threads.find((t) => t.entry.id === "plain")!)).toBeNull();
  });

  it("waypoints own the line: named stations, wagon at the current one, no plan ticks", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({
            ...charted,
            // A plan too — it must yield the line to the charted route.
            plan: { done: 3, total: 8, active: "Editing" },
          }),
        ],
      }),
    );
    const shape = trailShape(board.trains[0].threads[0]);
    expect(shape.ticks).toEqual([]);
    expect(shape.stations.map((s) => s.name)).toEqual(["define", "implement", "review"]);
    expect(shape.stations[0].done).toBe(true);
    // The wagon stands at the current station; the last station sits at camp.
    expect(shape.progress).toBeCloseTo(shape.stations[1].at);
    expect(shape.stations[2].at).toBeCloseTo(0.72);
  });

  it("with nothing current the wagon stands at the last done station", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({
            trail: {
              title: "t",
              waypoints: [
                { name: "define", status: "done" },
                { name: "implement", status: "ahead" },
                { name: "review", status: "ahead" },
              ],
            },
          }),
        ],
      }),
    );
    const shape = trailShape(board.trains[0].threads[0]);
    expect(shape.progress).toBeCloseTo(shape.stations[0].at);
  });
});

describe("the shipping stretch", () => {
  const route = (shipStatuses: [string, string][]) => ({
    title: "ship the ledger",
    waypoints: [
      { name: "implement", status: "done" as const },
      { name: "review", status: "done" as const },
      ...shipStatuses.map(([name, status]) => ({ name, status }) as never),
    ],
  });

  it("recognizes the shipping dialects models write", () => {
    expect(shipStage("pushed")).toBe("pushed");
    expect(shipStage("push")).toBe("pushed");
    expect(shipStage("code pushed")).toBe("pushed");
    expect(shipStage("pr-reviewed")).toBe("reviewed");
    expect(shipStage("PR reviewed")).toBe("reviewed");
    expect(shipStage("pull request reviewed")).toBe("reviewed");
    expect(shipStage("reviewed")).toBe("reviewed");
    expect(shipStage("merged")).toBe("merged");
    expect(shipStage("merge")).toBe("merged");
    expect(shipStage("pr merged")).toBe("merged");
    // Working stages never read as shipping.
    expect(shipStage("review")).toBeNull();
    expect(shipStage("implement")).toBeNull();
    expect(shipStage("plan")).toBeNull();
  });

  it("whole words only: working waypoints never read as ship gates", () => {
    // Substring matching once turned these into shipping stages — "approach"
    // and "approve" contain "pr", "unmerged" contains "merged".
    expect(shipStage("code review")).toBeNull();
    expect(shipStage("review approach")).toBeNull();
    expect(shipStage("approve review")).toBeNull();
    expect(shipStage("unmerged")).toBeNull();
    expect(shipStage("fix merge conflicts")).toBeNull();
    expect(shipStage("changes reviewed")).toBeNull();
  });

  it("un-done shipping waypoints gate the tie-off; done ones clear it", () => {
    const gated = deriveBoard(
      inputs({
        entries: [entry({ id: "g", trail: route([["pushed", "done"], ["pr-reviewed", "current"], ["merged", "ahead"]]) })],
      }),
    ).trains[0].threads[0];
    expect(gated.shipGates).toEqual(["reviewed", "merged"]);

    const clear = deriveBoard(
      inputs({
        entries: [entry({ id: "c", trail: route([["pushed", "done"], ["pr-reviewed", "done"], ["merged", "done"]]) })],
      }),
    ).trains[0].threads[0];
    expect(clear.shipGates).toEqual([]);

    // No charted trail: nothing was promised, nothing gates.
    const uncharted = deriveBoard(inputs({ entries: [entry({ id: "u" })] })).trains[0].threads[0];
    expect(uncharted.shipGates).toEqual([]);
  });

  it("shipping waypoints ride the camp → ring stretch; the wagon walks them", () => {
    const board = deriveBoard(
      inputs({
        entries: [entry({ id: "s", trail: route([["pushed", "done"], ["pr-reviewed", "current"], ["merged", "ahead"]]) })],
      }),
    );
    const shape = trailShape(board.trains[0].threads[0]);
    const ship = shape.stations.filter((st) => st.ship);
    const working = shape.stations.filter((st) => !st.ship);
    expect(ship.map((st) => st.name)).toEqual(["pushed", "pr-reviewed", "merged"]);
    // Working stations end at camp; shipping ones live past it.
    expect(Math.max(...working.map((st) => st.at))).toBeCloseTo(0.72);
    expect(ship.every((st) => st.at > 0.72 && st.at < 1)).toBe(true);
    // The wagon stands at the current shipping station.
    expect(shape.progress).toBeCloseTo(ship[1].at);
  });
});

describe("trail geometry", () => {
  const thread = (overrides: Partial<LedgerEntry> = {}, running = false) =>
    deriveBoard(
      inputs({
        entries: [entry(overrides)],
        running: running ? new Set([overrides.id ?? "s1"]) : new Set(),
      }),
    ).trains[0].threads[0];

  it("a planless camp thread waits at camp; settled reaches the ring", () => {
    expect(trailShape(thread()).progress).toBeCloseTo(0.72);
    const settled = deriveBoard(
      inputs({ entries: [entry({ settle: { settled_at: NOW, note: "" } })] }),
    ).settled[0];
    expect(trailShape(settled).progress).toBe(1);
  });

  it("a planned thread sits between its last done tick and the next", () => {
    const shape = trailShape(thread({ plan: { done: 2, total: 4, active: "Testing" } }));
    expect(shape.ticks).toHaveLength(4);
    expect(shape.ticksDone).toBe(2);
    expect(shape.progress).toBeGreaterThan(shape.ticks[1]);
    expect(shape.progress).toBeLessThan(shape.ticks[2]);
  });

  it("a finished plan parks the wagon at camp", () => {
    const shape = trailShape(thread({ plan: { done: 4, total: 4, active: null } }));
    expect(shape.progress).toBeCloseTo(0.72);
  });

  it("only routes whose last working station sits at camp drop the camp circle", () => {
    // Uncharted: the plain camp marker is the only landmark there.
    expect(trailShape(thread()).camp).toBe(true);
    // A standard charted route: the last working station IS camp.
    const standard = trailShape(
      thread({
        trail: {
          title: "t",
          waypoints: [
            { name: "implement", status: "done" },
            { name: "review", status: "current" },
            { name: "pushed", status: "ahead" },
          ],
        },
      }),
    );
    expect(standard.camp).toBe(false);
  });

  it("a ship-only route (a reopened thread) keeps its camp landmark and starts from camp", () => {
    const shape = trailShape(
      thread({
        trail: {
          title: "close the loops",
          waypoints: [
            { name: "pushed", status: "done" },
            { name: "pr-reviewed", status: "current" },
            { name: "merged", status: "ahead" },
          ],
        },
      }),
    );
    // No station occupies camp, so the circle still draws…
    expect(shape.camp).toBe(true);
    expect(shape.stations.every((s) => s.at > 0.72)).toBe(true);
    // …and the wagon never renders behind camp on a route that starts there.
    const untravelled = trailShape(
      thread({
        trail: {
          title: "close the loops",
          waypoints: [
            { name: "pushed", status: "ahead" },
            { name: "merged", status: "ahead" },
          ],
        },
      }),
    );
    expect(untravelled.progress).toBeCloseTo(0.72);
  });

  it("a degenerate interleaved route keeps station positions monotonic", () => {
    const shape = trailShape(
      thread({
        trail: {
          title: "t",
          waypoints: [
            { name: "implement", status: "done" },
            { name: "pushed", status: "done" },
            { name: "polish", status: "current" },
            { name: "merged", status: "ahead" },
          ],
        },
      }),
    );
    const ats = shape.stations.map((s) => s.at);
    expect([...ats].sort((a, b) => a - b)).toEqual(ats);
    // The wagon stands at the current station, never behind a done one.
    expect(shape.progress).toBeCloseTo(shape.stations[2].at);
    expect(shape.progress).toBeGreaterThan(shape.stations[1].at);
    // Ship styling survives the fallback layout.
    expect(shape.stations.map((s) => s.ship)).toEqual([false, true, false, true]);
  });
});
