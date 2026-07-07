import { describe, expect, it } from "vitest";
import { OxenTrailGame as G } from "./oregonTrail";

// Drive the state machine through handleKey the way the hero wrapper does.
function press(state: any, ...keys: string[]) {
  return keys.reduce((s, k) => G.handleKey(s, k), state);
}

describe("The Oxen Trail", () => {
  it("opens at the general store with money to spend", () => {
    const s: any = G.initialState();
    expect(s.phase).toBe("store");
    expect(s.storeMode).toBe("outfit");
    expect(s.spend.reduce((a: number, b: number) => a + b, 0)).toBeLessThanOrEqual(s.budget);
  });

  it("converts dollars into supplies and heads out on Enter", () => {
    const s: any = press(G.initialState(), "Enter");
    expect(s.phase).toBe("trail");
    expect(s.oxen).toBeGreaterThan(0);
    expect(s.food).toBeGreaterThan(0);
    expect(s.bullets).toBeGreaterThan(0);
    // Leftover money is kept as cash.
    expect(s.cash).toBeGreaterThanOrEqual(0);
  });

  it("blocks heading out without oxen", () => {
    let s: any = G.initialState();
    // Zero out the oxen row (row 0) with left presses, then try to leave.
    for (let i = 0; i < 40; i++) s = G.handleKey(s, "ArrowLeft");
    s = G.handleKey(s, "Enter");
    expect(s.phase).toBe("message");
    expect(s.msgTone).toBe("bad");
    // Dismissing returns to the store, not the trail.
    expect(G.handleKey(s, "Enter").phase).toBe("store");
  });

  it("advances miles and days when you continue on the trail", () => {
    const start: any = press(G.initialState(), "Enter");
    const after: any = G.handleKey(start, "1");
    expect(after.day).toBeGreaterThan(start.day);
    expect(after.miles).toBeGreaterThan(start.miles);
  });

  it("cycles pace and rations without leaving the trail", () => {
    const s: any = press(G.initialState(), "Enter");
    expect(G.handleKey(s, "3").pace).toBe((s.pace + 1) % 3);
    expect(G.handleKey(s, "4").rations).toBe((s.rations + 1) % 3);
    expect(G.handleKey(s, "3").phase).toBe("trail");
  });

  it("runs the hunting mini-game and rewards a correct fast word", () => {
    const trail: any = press(G.initialState(), "Enter");
    const hunt: any = G.handleKey(trail, "2");
    expect(hunt.phase).toBe("hunt");
    expect(hunt.huntWord.length).toBeGreaterThan(0);
    // Type the shown word; completing it resolves to a result message.
    const typed: any = press(hunt, ...hunt.huntWord.split(""));
    expect(typed.phase).toBe("message");
    expect(typed.food).toBeGreaterThan(trail.food);
    expect(typed.bullets).toBeLessThan(trail.bullets);
  });

  it("misses the hunt when it times out", () => {
    const hunt: any = G.handleKey(press(G.initialState(), "Enter"), "2");
    // Let the clock run past the limit with no correct keystrokes.
    const timedOut: any = G.update(hunt, 10);
    expect(timedOut.phase).toBe("message");
    expect(timedOut.msgTone).toBe("bad");
  });

  it("reaches Oregon and reports a score when the trail is done", () => {
    let s: any = press(G.initialState(), "Enter");
    // Continue (dismissing any message screens) until the journey ends.
    for (let i = 0; i < 400 && s.phase !== "over"; i++) {
      s = s.phase === "trail" ? G.handleKey(s, "1") : G.handleKey(s, s.phase === "river" ? "3" : "Enter");
    }
    expect(s.phase).toBe("over");
    expect(typeof s.score).toBe("number");
    // Either you made it or the party perished — both are terminal with a score.
    expect(s.arrived || s.cause.length > 0).toBe(true);
  });

  it("restarts a fresh outfit from the game-over screen", () => {
    const over: any = { ...G.initialState(), phase: "over", arrived: true, score: 999 };
    const again: any = G.handleKey(over, "Enter");
    expect(again.phase).toBe("store");
    expect(again.miles).toBe(0);
  });
});
