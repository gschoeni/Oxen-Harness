import { beforeEach, describe, expect, it } from "vitest";
import { loadTable, recordScore, wrapText } from "./terminal";

describe("terminal kit", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("wraps words to the column budget and keeps paragraphs", () => {
    expect(wrapText("the quick brown fox jumps", 10)).toEqual(["the quick", "brown fox", "jumps"]);
    expect(wrapText("one\ntwo three", 20)).toEqual(["one", "two three"]);
  });

  it("splits words longer than a line instead of overflowing", () => {
    expect(wrapText("supercalifragilistic", 8)).toEqual(["supercal", "ifragili", "stic"]);
  });

  it("truncates with an ellipsis past the line cap", () => {
    const lines = wrapText("a b c d e f g h", 3, 2);
    expect(lines).toHaveLength(2);
    expect(lines[1].endsWith("…")).toBe(true);
  });

  it("keeps a seeded top-5 per key and ranks a new run", () => {
    const seed = [
      { name: "A", score: 500, occupation: "x" },
      { name: "B", score: 300, occupation: "x" },
    ];
    expect(loadTable("t-scores", seed)).toEqual(seed);
    expect(recordScore("t-scores", seed, 400, "y")).toBe(2);
    expect(loadTable("t-scores", seed).map((e) => e.name)).toEqual(["A", "You", "B"]);
    expect(recordScore("t-scores", seed, 1, "y")).toBe(4);
  });
});
