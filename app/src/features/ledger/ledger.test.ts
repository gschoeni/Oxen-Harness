import { describe, expect, it } from "vitest";
import type { LedgerEntry, Project } from "../../lib/types";
import {
  ARCHIVE_DAYS,
  COLD_DAYS,
  currentStage,
  deriveBoard,
  RAIL_LIMIT,
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

describe("the attention rail", () => {
  it("ranks dangling above finished above open plans above going cold", () => {
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
    expect(board.rail.map((t) => t.entry.id)).toEqual(["dangle", "finished", "plan", "cold"]);
    expect(board.railOverflow).toBe(0);
  });

  it("caps the rail and reports the overflow", () => {
    const board = deriveBoard(
      inputs({
        entries: Array.from({ length: RAIL_LIMIT + 3 }, (_, i) =>
          entry({ id: `d${i}`, mid_turn: true }),
        ),
      }),
    );
    expect(board.rail).toHaveLength(RAIL_LIMIT);
    expect(board.railOverflow).toBe(3);
  });

  it("keeps running and content threads off the rail", () => {
    const board = deriveBoard(
      inputs({
        entries: [
          entry({ id: "busy", mid_turn: true }),
          entry({ id: "content", last_activity_at: NOW - 3_600 }),
        ],
        running: new Set(["busy"]),
      }),
    );
    expect(board.rail).toEqual([]);
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
});
