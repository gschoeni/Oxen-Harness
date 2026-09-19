import { describe, expect, it } from "vitest";
import type { ThreadEntry } from "../../lib/types";
import {
  needLabel,
  needRank,
  needsUser,
  openThreadIds,
  threadsFromState,
  workspaceVitals,
  type ThreadSlices,
} from "./threads";

const NOW = 1_753_000_000;
const DAY = 86_400;

function entry(overrides: Partial<ThreadEntry> = {}): ThreadEntry {
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
    finished_at: 0,
    review_status: "",
    seen_at: 0,
    ...overrides,
  };
}

function slices(entries: ThreadEntry[], overrides: Partial<ThreadSlices> = {}): ThreadSlices {
  return {
    threadsSnapshot: { entries, running: [] },
    runStatus: {},
    approvals: {},
    ...overrides,
  };
}

const approval = (session: string) => ({ session, id: "a1" }) as never;

describe("thread states", () => {
  it("classifies running, dangling, idle, and finished", () => {
    const threads = threadsFromState({
      threadsSnapshot: {
        entries: [
          entry({ id: "run", mid_turn: true }),
          entry({ id: "dangle", mid_turn: true }),
          entry({ id: "idle" }),
          entry({ id: "done", finished_at: NOW - 60 }),
        ],
        running: ["run"],
      },
      runStatus: {},
      approvals: {},
    });
    const states = Object.fromEntries([...threads.values()].map((t) => [t.entry.id, t.state]));
    expect(states).toEqual({ run: "running", dangle: "dangling", idle: "idle", done: "finished" });
  });

  it("unions the snapshot's running set with live run statuses", () => {
    const threads = threadsFromState(
      slices([entry({ id: "a" }), entry({ id: "b" })], {
        threadsSnapshot: { entries: [entry({ id: "a" }), entry({ id: "b" })], running: ["a"] },
        runStatus: { b: "running" },
      }),
    );
    expect(threads.get("a")?.state).toBe("running");
    expect(threads.get("b")?.state).toBe("running");
  });

  it("is empty until the first snapshot lands", () => {
    expect(threadsFromState({ threadsSnapshot: null, runStatus: {}, approvals: {} }).size).toBe(0);
  });

  it("marks a thread fresh only when it changed after its own seen mark and isn't running", () => {
    const threads = threadsFromState(
      slices([
        entry({ id: "unopened", last_activity_at: NOW - 3_600, seen_at: NOW - 2 * DAY }),
        entry({ id: "opened", last_activity_at: NOW - 3_600, seen_at: NOW - 1_800 }),
        entry({ id: "never", last_activity_at: NOW - 3_600, seen_at: 0 }),
        entry({ id: "busy", last_activity_at: NOW - 60, seen_at: NOW - DAY }),
      ], { runStatus: { busy: "running" } }),
    );
    expect(threads.get("unopened")?.fresh).toBe(true);
    expect(threads.get("unopened")?.need).toBe("unseen");
    expect(threads.get("opened")?.fresh).toBe(false);
    expect(threads.get("opened")?.need).toBeNull();
    // Never looked at: there is no baseline to be fresh against.
    expect(threads.get("never")?.fresh).toBe(false);
    expect(threads.get("busy")?.fresh).toBe(false);
  });
});

describe("stuck threads", () => {
  it("a running thread parked on an approval is stuck; an idle one with a stale approval is not", () => {
    const threads = threadsFromState(
      slices([entry({ id: "parked", mid_turn: true }), entry({ id: "busy", mid_turn: true }), entry({ id: "idle" })], {
        threadsSnapshot: {
          entries: [entry({ id: "parked", mid_turn: true }), entry({ id: "busy", mid_turn: true }), entry({ id: "idle" })],
          running: ["parked", "busy"],
        },
        approvals: { parked: approval("parked"), idle: approval("idle") },
      }),
    );
    expect(threads.get("parked")?.stuck).toBe(true);
    expect(threads.get("busy")?.stuck).toBe(false);
    expect(threads.get("idle")?.stuck).toBe(false);
  });
});

describe("needs: the one predicate the card pill and the chat list share", () => {
  it("names the reason in the user's words and ranks a parked agent first", () => {
    const entries = [
      entry({ id: "stuck", mid_turn: true }),
      entry({ id: "dangle", mid_turn: true }),
      entry({ id: "fin", last_activity_at: NOW - 600, seen_at: NOW - 3_600 }),
      entry({ id: "calm", last_activity_at: NOW - 2 * DAY }),
      entry({ id: "done", finished_at: NOW - 60 }),
      entry({ id: "busy", mid_turn: true }),
    ];
    const threads = threadsFromState({
      threadsSnapshot: { entries, running: ["stuck", "busy"] },
      runStatus: {},
      approvals: { stuck: approval("stuck") },
    });
    const t = (id: string) => threads.get(id)!;

    expect(needsUser(t("stuck"))).toBe(true);
    expect(needLabel(t("stuck"))).toBe("waiting on your approval");
    expect(needLabel(t("dangle"))).toMatch(/left dangling/);
    expect(needLabel(t("fin"))).toBe("finished while you were away");
    expect(needsUser(t("calm"))).toBe(false);
    expect(needLabel(t("calm"))).toBeNull();
    expect(needsUser(t("done"))).toBe(false);
    expect(needsUser(t("busy"))).toBe(false);

    const order = ["fin", "dangle", "stuck"]
      .map(t)
      .sort((a, b) => needRank(a) - needRank(b))
      .map((x) => x.entry.id);
    expect(order).toEqual(["stuck", "dangle", "fin"]);
    expect(needRank(t("calm"))).toBeGreaterThan(needRank(t("fin")));
  });
});

describe("a workspace's threads", () => {
  const entries = [
    entry({ id: "older", last_activity_at: NOW - 7_200 }),
    entry({ id: "newer" }),
    entry({ id: "dangling", mid_turn: true, last_activity_at: NOW - 10_000 }),
    entry({ id: "done", finished_at: NOW - 60 }),
    entry({ id: "running", mid_turn: true, last_activity_at: NOW - 60 }),
    entry({ id: "elsewhere", workspace: "/other", mid_turn: true }),
  ];
  const threads = threadsFromState({
    threadsSnapshot: { entries, running: ["running"] },
    runStatus: {},
    approvals: {},
  });

  it("opens in urgency order, then newest first, leaving finished ones out", () => {
    expect(openThreadIds(threads, "/work/app")).toEqual(["dangling", "running", "newer", "older"]);
    expect(openThreadIds(threads, "/nowhere")).toEqual([]);
  });

  it("folds the card's vital signs from the same verdicts", () => {
    expect(workspaceVitals(threads, "/work/app")).toEqual({ open: 4, running: 1, needs: 1 });
    expect(workspaceVitals(threads, "/other")).toEqual({ open: 1, running: 0, needs: 1 });
    expect(workspaceVitals(threads, "/nowhere")).toEqual({ open: 0, running: 0, needs: 0 });
  });
});
