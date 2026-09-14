import { beforeEach, describe, expect, it } from "vitest";
import { loadPref, seedRng } from "./gameKit";
import { TumbleweedDodgeGame as G, _dodge } from "./tumbleweed";

const { OX_ROW, HOP_TIME, READY_TIME, MULT_DECAY } = _dodge;

/** Advance the simulation `seconds` in small fixed steps like the raf loop. */
function run(state: any, seconds: number, step = 1 / 60) {
  let s = state;
  for (let t = 0; t < seconds; t += step) s = G.update(s, step);
  return s;
}

/** A run that is past the soft start, with no obstacles or pickups in play. */
function live() {
  let s: any = run(G.initialState(), READY_TIME + 0.05);
  return { ...s, obstacles: [], pickups: [], nextSpawn: 1e9, nextCoin: 1e9 };
}

function weed(lane: number, row: number, kind = "weed") {
  return { id: 9000 + row, kind, lane, laneVis: lane, row, speed: 1, spin: 0, bounced: false, bounceDir: 1, scored: false };
}

describe("Tumbleweed Dodge", () => {
  beforeEach(() => {
    seedRng(1);
    window.localStorage.clear();
  });

  it("soft-starts, then scores distance scaled by the multiplier", () => {
    const s0: any = G.initialState();
    expect(s0.phase).toBe("ready");
    expect(s0.score).toBe(0);
    const s1 = run(s0, READY_TIME + 1);
    expect(s1.phase).toBe("run");
    expect(s1.score).toBeGreaterThan(0);
  });

  it("changes lanes with a tween and buffers a second press", () => {
    let s = live();
    s = G.handleKey(s, "ArrowRight");
    expect(s.oxLane).toBe(2);
    // Mid-tween the ox hasn't arrived yet; a left press is queued, not lost.
    s = G.update(s, 0.02);
    expect(s.laneVis).toBeLessThan(2);
    s = G.handleKey(s, "ArrowLeft");
    expect(s.queuedLane).toBe(1);
    s = run(s, 0.3);
    expect(s.oxLane).toBe(1);
    expect(s.laneVis).toBe(1);
  });

  it("crashes into a weed in its lane, with hit-stop and a card", () => {
    let s = live();
    s = { ...s, obstacles: [weed(1, OX_ROW - 6)] };
    for (let i = 0; i < 90 && s.phase !== "dead"; i++) s = G.update(s, 1 / 60);
    expect(s.phase).toBe("dead");
    expect(s.shake).toBeGreaterThan(0);
    expect(s.hitStop).toBeGreaterThan(0);
    s = run(s, 1);
    // The key that was being mashed at the crash doesn't restart instantly...
    expect(G.handleKey({ ...s, deadFor: 0.1 }, "ArrowLeft").phase).toBe("dead");
    // ...but any arrow does once the card is up.
    const again = G.handleKey({ ...s, deadFor: 1 }, "ArrowLeft");
    expect(again.phase).toBe("ready");
    expect(again.best).toBeGreaterThanOrEqual(0);
  });

  it("hops over a weed and earns a multiplier bump", () => {
    let s = live();
    s = { ...s, obstacles: [weed(1, OX_ROW - 3)] };
    s = G.handleKey(s, "ArrowUp");
    expect(s.hop).toBeCloseTo(HOP_TIME);
    s = run(s, 0.3);
    expect(s.phase).toBe("run");
    expect(s.mult).toBe(2);
    expect(s.popups.some((p: any) => p.text.startsWith("HOPPED"))).toBe(true);
  });

  it("a log must be hopped: it hits in any lane", () => {
    let s = live();
    s = { ...s, oxLane: 0, laneVis: 0, obstacles: [weed(1, OX_ROW - 5, "log")] };
    expect(run(s, 1).phase).toBe("dead");
    const hopped = run(G.handleKey({ ...s, obstacles: [weed(1, OX_ROW - 3, "log")] }, "ArrowUp"), 0.3);
    expect(hopped.phase).toBe("run");
  });

  it("rewards a graze in the neighbouring lane and decays the multiplier", () => {
    let s = live();
    s = { ...s, obstacles: [weed(2, OX_ROW - 1)] };
    s = run(s, 0.25);
    expect(s.mult).toBe(2);
    expect(s.popups.some((p: any) => p.text.startsWith("CLOSE"))).toBe(true);
    s = run({ ...s, obstacles: [] }, MULT_DECAY + 0.2);
    expect(s.mult).toBe(1);
  });

  it("a horseshoe shield absorbs one crash", () => {
    let s = live();
    s = { ...s, shield: true, obstacles: [weed(1, OX_ROW - 4)] };
    s = run(s, 1);
    expect(s.phase).toBe("run");
    expect(s.shield).toBe(false);
    expect(s.obstacles.length).toBe(0);
  });

  it("chains coins into a multiplier bump every five", () => {
    let s = live();
    const coins = [0, 1, 2, 3, 4].map((i) => ({ id: 100 + i, kind: "coin", lane: 1, row: OX_ROW - 3 - i * 2.2 }));
    s = { ...s, pickups: coins };
    s = run(s, 1.5);
    expect(s.pickups.filter((p: any) => p.kind === "coin").length).toBe(0);
    expect(s.coinChain).toBe(5);
    expect(s.mult).toBe(2);
  });

  it("never spawns something in every lane at once", () => {
    let s: any = G.initialState();
    for (let i = 0; i < 60 * 40; i++) {
      s = G.update(s, 1 / 60);
      if (s.phase === "dead") s = G.handleKey({ ...s, deadFor: 1 }, "ArrowLeft");
      const fresh = s.obstacles.filter((o: any) => o.row < _dodge.HZ + 5 && o.kind !== "log");
      expect(new Set(fresh.map((o: any) => o.lane)).size).toBeLessThan(3);
    }
  });

  it("raises a landmark banner as yards pile up", () => {
    let s = live();
    s = { ...s, dist: 299 };
    s = run(s, 0.5);
    expect(s.milestone).toBe(1);
    expect(s.banner?.text).toBe("FORT KEARNEY");
  });

  it("queues sound cues for coins and the crash", () => {
    let s = live();
    s = { ...s, pickups: [{ id: 1, kind: "coin", lane: 1, row: OX_ROW - 2 }] };
    s = run(s, 0.3);
    expect(s.sfx.map((e: any) => e.name)).toContain("coin");
    s = { ...s, obstacles: [weed(1, OX_ROW - 2)] };
    for (let i = 0; i < 90 && s.phase !== "dead"; i++) s = G.update(s, 1 / 60);
    expect(s.phase).toBe("dead");
    expect(s.sfx[s.sfx.length - 1].name).toBe("crash");
  });

  it("leaves a wreck on the trail after a restart", () => {
    let s = live();
    s = { ...s, oxLane: 2, laneVis: 2, obstacles: [weed(2, OX_ROW - 2)] };
    for (let i = 0; i < 90 && s.phase !== "dead"; i++) s = G.update(s, 1 / 60);
    const again = G.handleKey({ ...s, deadFor: 1 }, "ArrowLeft");
    expect(again.phase).toBe("ready");
    expect(again.wrecks).toEqual([{ lane: 2, row: OX_ROW }]);
    // It scrolls off with the trail rather than lingering forever.
    const later = run(again, 3);
    expect(later.wrecks.length).toBe(0);
  });

  it("unlocks the mule at 1000 yards and saves it", () => {
    let s = live();
    s = { ...s, dist: 999 };
    s = run(s, 0.5);
    expect(s.unlocked).toContain("mule");
    expect(loadPref("dodge-unlocks")).toContain("mule");
    expect(s.popups.some((p: any) => p.text === "MULE UNLOCKED!")).toBe(true);
    expect(s.sfx.some((e: any) => e.name === "best")).toBe(true);
  });

  it("chooses a rider from the attract screen only once unlocked", () => {
    const s0: any = G.initialState();
    expect(G.handleAttractKey!(s0, "2").rider).toBe("ox");
    const withMule = { ...s0, unlocked: ["ox", "mule"] };
    const chosen = G.handleAttractKey!(withMule, "2");
    expect(chosen.rider).toBe("mule");
    expect(loadPref("dodge-rider")).toBe("mule");
    // The choice survives a fresh run.
    expect((G.initialState() as any).rider).toBe("ox"); // unlocks pref isn't set, so it falls back
  });

  it("keeps the rider once both prefs are stored", () => {
    window.localStorage.setItem("oxen-hero-dodge-unlocks", "ox,mule,wagon");
    window.localStorage.setItem("oxen-hero-dodge-rider", "wagon");
    expect((G.initialState() as any).rider).toBe("wagon");
  });

  it("tightens the spawn gap floor after wave 8", () => {
    let s = live();
    s = { ...s, wave: 10, waveT: 11, nextSpawn: s.time, obstacles: [] };
    const next = G.update(s, 1 / 60);
    expect(next.obstacles.length).toBeGreaterThan(0);
    // floor at wave 10 is 0.42 - 0.08*(2/6) ≈ 0.393, plus up to 0.35 jitter;
    // pairs and logs stretch it by at most 1.8×.
    expect(next.nextSpawn - next.time).toBeLessThanOrEqual((0.34 + 0.35) * 1.8 + 0.02);
  });

  it("restarts from a tap once the grace period has passed", () => {
    let s = live();
    s = { ...s, obstacles: [weed(1, OX_ROW - 2)] };
    for (let i = 0; i < 90 && s.phase !== "dead"; i++) s = G.update(s, 1 / 60);
    expect(G.handlePointer!({ ...s, deadFor: 0.1 }, { kind: "tap", x: 0.5, y: 0.5 }).phase).toBe("dead");
    expect(G.handlePointer!({ ...s, deadFor: 1 }, { kind: "tap", x: 0.5, y: 0.5 }).phase).toBe("ready");
  });

  it("maps taps and swipes onto the arrow controls", () => {
    let s = live();
    expect(G.handlePointer!(s, { kind: "tap", x: 0.9, y: 0.8 }).oxLane).toBe(2);
    expect(G.handlePointer!(s, { kind: "tap", x: 0.5, y: 0.1 }).hop).toBeCloseTo(HOP_TIME);
    expect(G.handlePointer!(s, { kind: "swipe", x: 0.5, y: 0.5, dir: "ArrowLeft" }).oxLane).toBe(0);
  });
});
