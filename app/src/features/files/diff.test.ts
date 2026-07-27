import { describe, expect, it } from "vitest";
import { diffTab, diffTarget, isDiffPath, parseUnifiedDiff } from "./diff";

describe("diff tabs", () => {
  it("round-trips the marker and never mistakes media for diffs", () => {
    const tab = diffTab("src/photo.png");
    expect(isDiffPath(tab)).toBe(true);
    expect(diffTarget(tab)).toBe("src/photo.png");
    expect(isDiffPath("src/photo.png")).toBe(false);
  });
});

describe("parseUnifiedDiff", () => {
  const SAMPLE = [
    "diff --git a/src/main.rs b/src/main.rs",
    "index 3b18e51..1c8b6f9 100644",
    "--- a/src/main.rs",
    "+++ b/src/main.rs",
    "@@ -1,3 +1,4 @@",
    " fn main() {",
    '-    println!("hi");',
    '+    println!("hello");',
    '+    println!("world");',
    " }",
    "",
  ].join("\n");

  it("numbers old and new lines through a hunk", () => {
    const rows = parseUnifiedDiff(SAMPLE);
    const kinds = rows.map((r) => r.kind);
    expect(kinds).toEqual(["meta", "meta", "meta", "meta", "hunk", "ctx", "del", "add", "add", "ctx"]);

    const del = rows.find((r) => r.kind === "del")!;
    expect(del).toMatchObject({ oldNo: 2, newNo: null, text: '    println!("hi");' });
    const adds = rows.filter((r) => r.kind === "add");
    expect(adds.map((r) => r.newNo)).toEqual([2, 3]);
    const closing = rows[rows.length - 1];
    expect(closing).toMatchObject({ kind: "ctx", oldNo: 3, newNo: 4, text: "}" });
  });

  it("keeps counting correctly across multiple hunks and files", () => {
    const multi = [
      "@@ -10,2 +20,2 @@",
      " a",
      "-b",
      "+B",
      "diff --git a/x b/x",
      "@@ -1 +1 @@",
      "-old",
      "+new",
    ].join("\n");
    const rows = parseUnifiedDiff(multi);
    expect(rows.find((r) => r.text === "B")).toMatchObject({ newNo: 21 });
    expect(rows.find((r) => r.text === "new")).toMatchObject({ newNo: 1 });
    expect(rows.find((r) => r.text === "old")).toMatchObject({ oldNo: 1 });
  });

  it("treats an empty diff as all-meta (the viewer's 'no changes' state)", () => {
    expect(parseUnifiedDiff("").every((r) => r.kind === "meta")).toBe(true);
    expect(parseUnifiedDiff("").length).toBe(0);
  });

  it("passes the no-newline marker through as meta", () => {
    const rows = parseUnifiedDiff("@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file");
    expect(rows[rows.length - 1]).toMatchObject({ kind: "meta" });
  });
});
