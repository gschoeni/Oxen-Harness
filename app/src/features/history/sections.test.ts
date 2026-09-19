import { describe, expect, it } from "vitest";
import { matchesQuery, sectionRows } from "./sections";
import type { SessionSummary, ThreadEntry } from "../../lib/types";
import { threadsFromState } from "../threads/threads";

const row: SessionSummary = {
  id: "abc123",
  workspace: "/Users/greg/Code/AI/OxenHarness",
  model: "anthropic/claude-sonnet-4-5",
  created_at: 1,
  title: "Fix the flaky parser",
  message_count: 2,
  review_status: "",
  source: "",
};

describe("matchesQuery", () => {
  it("matches every term against title, model, project, folder, and id, case-blind", () => {
    expect(matchesQuery(row, "Oxen Harness", "")).toBe(true);
    expect(matchesQuery(row, "Oxen Harness", "flaky PARSER")).toBe(true);
    expect(matchesQuery(row, "Oxen Harness", "sonnet")).toBe(true);
    expect(matchesQuery(row, "Oxen Harness", "harness")).toBe(true);
    expect(matchesQuery(row, "Oxen Harness", "abc1")).toBe(true);
    expect(matchesQuery(row, "Oxen Harness", "flaky tests")).toBe(false);
  });

  it("ignores the folder's full path, so a home directory doesn't match everything", () => {
    expect(matchesQuery(row, "Oxen Harness", "greg")).toBe(false);
    expect(matchesQuery(row, "Oxen Harness", "code")).toBe(false);
  });
});

describe("sectionRows", () => {
  const NOW = 1_753_000_000;
  const entry = (id: string, overrides: Partial<ThreadEntry> = {}): ThreadEntry => ({
    id,
    workspace: "/w",
    model: "m",
    created_at: NOW - 7_200,
    last_activity_at: NOW - 600,
    title: id,
    last_reply: "",
    message_count: 2,
    mid_turn: false,
    finished_at: 0,
    review_status: "",
    seen_at: 0,
    ...overrides,
  });
  const summary = (id: string): SessionSummary => ({ ...row, id, title: id });

  it("bands needs (most urgent first), open chats, then finished ones", () => {
    const threads = threadsFromState({
      threadsSnapshot: {
        entries: [
          entry("calm"),
          entry("unseen", { seen_at: NOW - 3_600 }),
          entry("dangling", { mid_turn: true }),
          entry("stuck", { mid_turn: true }),
          entry("done", { finished_at: NOW - 60 }),
        ],
        running: ["stuck"],
      },
      runStatus: { offscreen: "unread" },
      approvals: { stuck: { id: "a", session: "stuck" } as never },
    });
    const rows = ["calm", "unseen", "dangling", "stuck", "done", "offscreen", "unknown"].map(summary);
    const sections = sectionRows(rows, threads, { offscreen: "unread" });
    expect(sections.needs.map((n) => n.row.id)).toEqual(["stuck", "dangling", "unseen", "offscreen"]);
    expect(sections.needs.map((n) => n.label)).toEqual([
      "waiting on your approval",
      "left dangling — reply never arrived",
      "finished while you were away",
      "finished while you were away",
    ]);
    expect(sections.open.map((r) => r.id)).toEqual(["calm", "unknown"]);
    expect(sections.finished.map((r) => r.id)).toEqual(["done"]);
  });
});
