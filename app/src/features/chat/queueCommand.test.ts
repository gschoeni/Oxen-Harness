import { describe, expect, it } from "vitest";
import { queueCommand } from "./queueCommand";

describe("desktop queue slash command", () => {
  it("adds and edits without sending", () => {
    expect(queueCommand("add third task", ["first"])).toMatchObject({
      kind: "update",
      items: ["first", "third task"],
    });
    expect(queueCommand("edit 1 revised task", ["first", "second"])).toMatchObject({
      kind: "update",
      items: ["revised task", "second"],
    });
  });

  it("reorders, removes, clears, and starts a drain", () => {
    expect(queueCommand("up 2", ["a", "b"])).toMatchObject({ items: ["b", "a"] });
    expect(queueCommand("rm 1", ["a", "b"])).toMatchObject({ items: ["b"] });
    expect(queueCommand("clear", ["a"])).toMatchObject({ items: [] });
    expect(queueCommand("run", ["a", "b"])).toEqual({ kind: "run", items: ["a", "b"] });
  });

  it("rejects invalid positions and empty runs", () => {
    expect(queueCommand("down 2", ["a", "b"]).kind).toBe("error");
    expect(queueCommand("run", []).kind).toBe("error");
  });
});
