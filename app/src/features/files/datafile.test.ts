import { describe, expect, it } from "vitest";
import {
  PAGE_SIZE,
  editText,
  formatCell,
  gutterWidth,
  initialWidth,
  isDataPath,
  pageOf,
  parseEdit,
} from "./datafile";

describe("isDataPath", () => {
  it("routes the data extensions to the grid, case-insensitively", () => {
    expect(isDataPath("train.csv")).toBe(true);
    expect(isDataPath("data/Events.JSONL")).toBe(true);
    expect(isDataPath("x.ndjson")).toBe(true);
    expect(isDataPath("x.tsv")).toBe(true);
    expect(isDataPath("weights/model.parquet")).toBe(true);
    expect(isDataPath("README.md")).toBe(false);
    expect(isDataPath("csv")).toBe(false);
    expect(isDataPath("archive.csv.gz")).toBe(false);
  });
});

describe("pageOf", () => {
  it("maps view rows onto backend pages", () => {
    expect(pageOf(0)).toBe(0);
    expect(pageOf(PAGE_SIZE - 1)).toBe(0);
    expect(pageOf(PAGE_SIZE)).toBe(1);
    expect(pageOf(5 * PAGE_SIZE + 3)).toBe(5);
  });
});

describe("formatCell / editText", () => {
  it("shows floats at a sane width but edits them exactly", () => {
    expect(formatCell(0.30000000000000004, "float")).toBe("0.3");
    expect(editText(0.30000000000000004)).toBe("0.30000000000000004");
    expect(formatCell(2, "float")).toBe("2");
  });
  it("renders null as empty and booleans as words", () => {
    expect(formatCell(null, "str")).toBe("");
    expect(formatCell(true, "bool")).toBe("true");
    expect(editText(null)).toBe("");
  });
});

describe("parseEdit", () => {
  it("parses by column kind and rejects what doesn't fit", () => {
    expect(parseEdit("42", "int")).toBe(42);
    expect(parseEdit("4.2", "int")).toBeUndefined();
    expect(parseEdit("4.2", "float")).toBe(4.2);
    expect(parseEdit("abc", "float")).toBeUndefined();
    expect(parseEdit("yes", "bool")).toBe(true);
    expect(parseEdit("FALSE", "bool")).toBe(false);
    expect(parseEdit("maybe", "bool")).toBeUndefined();
    expect(parseEdit("hello", "str")).toBe("hello");
  });
  it("treats an emptied cell as null", () => {
    expect(parseEdit("", "int")).toBeNull();
    expect(parseEdit("  ", "float")).toBeNull();
    expect(parseEdit("", "str")).toBeNull();
  });
  it("keeps nested values read-only", () => {
    expect(parseEdit("[1,2]", "list")).toBeUndefined();
    expect(parseEdit("{}", "struct")).toBeUndefined();
  });
});

describe("sizing", () => {
  it("clamps column widths to a readable range", () => {
    expect(initialWidth("x", "str", [])).toBeGreaterThanOrEqual(88);
    expect(initialWidth("a".repeat(400), "str", [])).toBeLessThanOrEqual(360);
  });
  it("widens the gutter as row numbers grow", () => {
    expect(gutterWidth(99)).toBeLessThan(gutterWidth(9_999_999));
  });
});
