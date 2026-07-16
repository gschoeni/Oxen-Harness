import { describe, expect, it } from "vitest";
import { fenceHint, snippetLabel, withSnippetContext } from "./snippets";

describe("fenceHint", () => {
  it("uses the extension as the markdown fence hint", () => {
    expect(fenceHint("src/lib/store.ts")).toBe("ts");
    expect(fenceHint("app/Main.RS")).toBe("rs");
  });

  it("returns no hint for extensionless or dotfiles", () => {
    expect(fenceHint("Makefile")).toBe("");
    expect(fenceHint(".gitignore")).toBe("");
  });
});

describe("snippetLabel", () => {
  it("shows a line range, collapsing single lines", () => {
    expect(snippetLabel({ path: "a/b.ts", start: 10, end: 24, code: "" })).toBe("a/b.ts:10-24");
    expect(snippetLabel({ path: "a/b.ts", start: 7, end: 7, code: "" })).toBe("a/b.ts:7");
  });
});

describe("withSnippetContext", () => {
  it("passes text through when nothing is staged", () => {
    expect(withSnippetContext("fix it", [])).toBe("fix it");
  });

  it("prefixes each snippet as a cited fenced block", () => {
    const prompt = withSnippetContext("Why does this loop twice?", [
      { path: "src/a.ts", start: 3, end: 4, code: "for (;;) {}\ndone()" },
      { path: "src/b.rs", start: 1, end: 1, code: "fn main() {}" },
    ]);
    expect(prompt).toBe(
      "Context from `src/a.ts` (lines 3-4):\n\n```ts\nfor (;;) {}\ndone()\n```\n\n" +
        "Context from `src/b.rs` (lines 1-1):\n\n```rs\nfn main() {}\n```\n\n" +
        "Why does this loop twice?",
    );
  });
});
