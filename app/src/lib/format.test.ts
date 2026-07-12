import { describe, expect, it } from "vitest";
import { relPath } from "./format";

describe("relPath", () => {
  it("strips the project root prefix", () => {
    expect(relPath("/home/me/proj/src/main.rs", "/home/me/proj")).toBe("src/main.rs");
  });

  it("tolerates a trailing separator on the root", () => {
    expect(relPath("/home/me/proj/src/main.rs", "/home/me/proj/")).toBe("src/main.rs");
  });

  it("returns the basename when the path IS the root", () => {
    expect(relPath("/home/me/proj", "/home/me/proj")).toBe("proj");
  });

  it("keeps the full path when it is outside the project", () => {
    expect(relPath("/etc/hosts", "/home/me/proj")).toBe("/etc/hosts");
  });

  it("does not treat a sibling with a shared prefix as inside", () => {
    expect(relPath("/home/me/proj-other/x", "/home/me/proj")).toBe("/home/me/proj-other/x");
  });

  it("returns the path unchanged when the root is unknown", () => {
    expect(relPath("/home/me/proj/src/main.rs", null)).toBe("/home/me/proj/src/main.rs");
    expect(relPath("relative/path.ts")).toBe("relative/path.ts");
  });

  it("handles Windows-style separators", () => {
    expect(relPath("C:\\proj\\src\\main.rs", "C:\\proj")).toBe("src\\main.rs");
  });
});
