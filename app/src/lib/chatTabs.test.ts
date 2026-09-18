import { describe, expect, it } from "vitest";
import {
  movedTab,
  neighbourTab,
  parseChatTabs,
  stripOf,
  withTab,
  withoutStrip,
  withoutTabs,
} from "./chatTabs";

const tabs = { "/w": ["a", "b", "c"], "/other": ["x"] };

describe("chatTabs", () => {
  it("parses only the expected shape from a persisted file", () => {
    expect(parseChatTabs(null)).toEqual({});
    expect(parseChatTabs(["a"])).toEqual({});
    expect(parseChatTabs({ "/w": ["a", 1], "/ok": ["b"], "/bad": "b" })).toEqual({ "/ok": ["b"] });
  });

  it("opens a tab once, at the end, and leaves the record alone when it's there", () => {
    expect(withTab(tabs, "/w", "d")["/w"]).toEqual(["a", "b", "c", "d"]);
    expect(withTab(tabs, "/new", "d")["/new"]).toEqual(["d"]);
    expect(withTab(tabs, "/w", "b")).toBe(tabs);
  });

  it("closes tabs across strips, dropping a strip left empty", () => {
    expect(withoutTabs(tabs, ["b", "x"])).toEqual({ "/w": ["a", "c"] });
    expect(withoutTabs(tabs, ["nope"])).toBe(tabs);
  });

  it("drops a whole strip when its project goes", () => {
    expect(withoutStrip(tabs, "/other")).toEqual({ "/w": ["a", "b", "c"] });
    expect(withoutStrip(tabs, "/missing")).toBe(tabs);
  });

  it("moves a tab before another, or to the end, within its own strip", () => {
    expect(movedTab(tabs, "c", "a")["/w"]).toEqual(["c", "a", "b"]);
    expect(movedTab(tabs, "a", null)["/w"]).toEqual(["b", "c", "a"]);
    expect(movedTab(tabs, "a", "b")).toBe(tabs); // already there
    expect(movedTab(tabs, "a", "a")).toBe(tabs);
    expect(movedTab(tabs, "a", "x")).toBe(tabs); // another strip
    expect(movedTab(tabs, "nope", null)).toBe(tabs);
  });

  it("lands on the right neighbour, else the left, else nowhere", () => {
    expect(neighbourTab(["a", "b", "c"], "b")).toBe("c");
    expect(neighbourTab(["a", "b", "c"], "c")).toBe("b");
    expect(neighbourTab(["a"], "a")).toBeNull();
    expect(neighbourTab(["a"], "z")).toBeNull();
  });

  it("finds the strip a tab lives in", () => {
    expect(stripOf(tabs, "x")).toEqual(["/other", ["x"]]);
    expect(stripOf(tabs, "nope")).toBeUndefined();
  });
});
