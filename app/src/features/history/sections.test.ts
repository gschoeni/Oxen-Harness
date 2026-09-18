import { describe, expect, it } from "vitest";
import { matchesQuery } from "./sections";
import type { SessionSummary } from "../../lib/types";

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
