import { describe, expect, it } from "vitest";
import { tabSignals, tabStatus, wantsUser, type TabSignals } from "./tabStatus";
import type { Thread } from "../ledger/ledger";
import type { LedgerEntry } from "../../lib/types";

const entry: LedgerEntry = {
  id: "s1",
  workspace: "/w",
  model: "m",
  created_at: 0,
  last_activity_at: 0,
  title: "",
  last_reply: "",
  message_count: 2,
  mid_turn: false,
  plan: { done: 1, total: 3, active: null },
  trail: null,
  settle: null,
  review_status: "",
  seen_at: 0,
};

const thread = (overrides: Partial<Thread>): Thread => ({
  entry,
  state: "camp",
  shipGates: [],
  stuck: false,
  fresh: false,
  idleDays: 0,
  weather: 0,
  need: null,
  ...overrides,
});

const quiet: TabSignals = { running: false, unread: false, approval: false, question: false, thread: null };

describe("tabStatus", () => {
  it("is idle when nothing is owed, even with no board verdict yet", () => {
    expect(tabStatus(quiet)).toEqual({ state: "idle", label: null });
  });

  it("a pending approval outranks everything, running included", () => {
    const status = tabStatus({ ...quiet, running: true, approval: true });
    expect(status.state).toBe("needs");
    expect(status.label).toMatch(/approval/);
    // The board's own parked flag says the same thing.
    expect(tabStatus({ ...quiet, thread: thread({ state: "running", stuck: true }) }).state).toBe("needs");
  });

  it("a waiting question is a claim on the user", () => {
    expect(tabStatus({ ...quiet, question: true })).toEqual({ state: "needs", label: "asked you a question" });
  });

  it("a live turn outranks the board's stale verdicts", () => {
    // The snapshot still says the reply never arrived; the stream says otherwise.
    const stale = thread({ state: "dangling", need: "dangling" });
    expect(tabStatus({ ...quiet, running: true, thread: stale }).state).toBe("running");
    // And the board's running set counts when the live flag hasn't caught up.
    expect(tabStatus({ ...quiet, thread: thread({ state: "running" }) }).state).toBe("running");
  });

  it("a lost reply is broken, wearing the board's reason", () => {
    const status = tabStatus({ ...quiet, thread: thread({ state: "dangling", need: "dangling" }) });
    expect(status.state).toBe("broken");
    expect(status.label).toMatch(/dangling/);
  });

  it("an open plan needs the user once the chat is idle", () => {
    const status = tabStatus({ ...quiet, thread: thread({ need: "plan-open" }) });
    expect(status.state).toBe("needs");
    expect(status.label).toBe("plan 1/3 — pick it up or tie off");
  });

  it("a finish nobody looked at is something to check, from either source", () => {
    expect(tabStatus({ ...quiet, unread: true }).state).toBe("check");
    expect(tabStatus({ ...quiet, thread: thread({ need: "finished", fresh: true }) }).state).toBe("check");
  });

  it("going cold is the history's nag, not the open tab's", () => {
    expect(tabStatus({ ...quiet, thread: thread({ need: "going-cold", idleDays: 9 }) }).state).toBe("idle");
  });

  it("gathers a chat's signals from the store slices by id", () => {
    const store = {
      runStatus: { a: "running" as const, b: "unread" as const },
      approvals: { b: { session: "b" } as never },
      question: { session: "c", id: "q", questions: [] },
    };
    expect(tabSignals(store, "a", null)).toMatchObject({ running: true, unread: false, approval: false, question: false });
    expect(tabSignals(store, "b", null)).toMatchObject({ running: false, unread: true, approval: true });
    expect(tabSignals(store, "c", null)).toMatchObject({ question: true });
  });

  it("the history badge counts needs, broken, and check — not running or idle", () => {
    expect(wantsUser({ state: "needs", label: "" })).toBe(true);
    expect(wantsUser({ state: "broken", label: "" })).toBe(true);
    expect(wantsUser({ state: "check", label: "" })).toBe(true);
    expect(wantsUser({ state: "running", label: "" })).toBe(false);
    expect(wantsUser({ state: "idle", label: null })).toBe(false);
  });
});
