import { describe, expect, it } from "vitest";
import { canvasDocFromArgs, formatForPath, slugId } from "./canvas";

describe("canvasDocFromArgs", () => {
  it("rebuilds a content doc with the backend's id derivation", () => {
    const doc = canvasDocFromArgs({ title: "Q3 Launch Plan!", format: "markdown", content: "# Plan" });
    expect(doc).toEqual({
      id: "q3-launch-plan",
      title: "Q3 Launch Plan!",
      format: "markdown",
      language: undefined,
      content: "# Plan",
      path: null,
    });
    expect(canvasDocFromArgs({ format: "markdown", content: "   " })).toBeNull();
  });

  it("rebuilds a project-file doc from its path alone", () => {
    // A call that showed a file carries no content: the panel reads the file.
    const doc = canvasDocFromArgs({ path: "./site/index.html" });
    expect(doc).toEqual({
      id: slugId("site/index.html"),
      title: "index.html",
      format: "html",
      language: undefined,
      content: "",
      path: "site/index.html",
    });
    expect(doc!.id).toBe("site-index-html");
    expect(canvasDocFromArgs({ path: "notes.md", title: "Notes" })!.title).toBe("Notes");
    expect(formatForPath("a/b/main.rs")).toBe("code");
    expect(formatForPath("logo.SVG")).toBe("svg");
  });
});
