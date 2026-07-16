import { describe, expect, it } from "vitest";
import { CHAT_MIN_FIT, planColumns, RAIL_W, type ColumnInput } from "./layout";

const column = (over: Partial<ColumnInput> = {}): ColumnInput => ({
  available: true,
  collapsed: false,
  desired: 400,
  min: 300,
  ...over,
});

describe("planColumns", () => {
  it("keeps chosen widths when the window has room", () => {
    const plan = planColumns(2000, column(), column());
    expect(plan.left).toEqual({ width: 400, railed: false });
    expect(plan.right).toEqual({ width: 400, railed: false });
    expect(plan.chatRailed).toBe(false);
  });

  it("returns no column for an empty side", () => {
    const plan = planColumns(1000, column({ available: false }), column());
    expect(plan.left).toBeNull();
    expect(plan.right).toEqual({ width: 400, railed: false });
  });

  it("respects a user-collapsed side (rail width, still railed)", () => {
    const plan = planColumns(2000, column({ collapsed: true }), column());
    expect(plan.left).toEqual({ width: RAIL_W, railed: true });
  });

  it("shrinks the right column toward its minimum before touching the left", () => {
    // 400 + 400 + chat: 60px short of the chat minimum.
    const plan = planColumns(400 + 400 + CHAT_MIN_FIT - 60, column(), column());
    expect(plan.right).toEqual({ width: 340, railed: false });
    expect(plan.left).toEqual({ width: 400, railed: false });
    expect(plan.chatRailed).toBe(false);
  });

  it("rails the right column when shrinking is not enough", () => {
    const plan = planColumns(900, column(), column());
    // Both shrink to their minimum first (still short), then the right rails.
    expect(plan.right).toEqual({ width: RAIL_W, railed: true });
    expect(plan.left?.railed).toBe(false);
    expect(900 - plan.left!.width - RAIL_W).toBeGreaterThanOrEqual(CHAT_MIN_FIT);
  });

  it("rails both columns before giving up on the chat", () => {
    const plan = planColumns(450, column(), column());
    expect(plan.left).toEqual({ width: RAIL_W, railed: true });
    expect(plan.right).toEqual({ width: RAIL_W, railed: true });
    expect(plan.chatRailed).toBe(false); // 450 - 104 = 346 ≥ the chat-rail floor
  });

  it("rails the chat itself only in the terminal squeeze", () => {
    const plan = planColumns(300, column(), column());
    expect(plan.left?.railed).toBe(true);
    expect(plan.right?.railed).toBe(true);
    expect(plan.chatRailed).toBe(true);
  });
});
