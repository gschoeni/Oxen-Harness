import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ANIMALS, HuntGame as G, newTrip, quotaFor, terrainForMiles, tripKey, tripKeyUp, tripPointer, tripResult, tripUpdate, type HuntTrip } from "./hunt";
import { COLS, ROWS, seedRng } from "./gameKit";

// Step a trip through the frame loop the way the hero wrapper does.
function run(t: HuntTrip, seconds: number, dt = 1 / 60): HuntTrip {
  for (let s = 0; s < seconds; s += dt) t = tripUpdate(t, dt);
  return t;
}

// A quiet field: no obstacles, no animals arriving, ready to walk.
function quiet(over: Partial<HuntTrip> = {}): HuntTrip {
  return { ...newTrip({ terrain: "plains", bullets: 10, seconds: 30, carryCap: 100 }), obstacles: [], nextSpawn: 999, ...over };
}

function deerAt(x: number, y: number) {
  return { id: 99, kind: "deer" as const, x, y, vx: 0, vy: 0, hp: 1, dead: false, timer: 99, moving: false, age: 0, spooked: false, tell: 0, charging: false, rear: 0, retarget: 0 };
}

beforeEach(() => seedRng(42));
afterEach(() => vi.restoreAllMocks());

describe("a hunting trip", () => {
  it("drops the hunter mid-field among the zone's objects", () => {
    const t = newTrip({ terrain: "mountains", bullets: 20, seconds: 30, carryCap: 100 });
    expect(t.hunter.x).toBe(COLS / 2);
    expect(t.hunter.y).toBe(ROWS / 2);
    expect(t.obstacles.length).toBeGreaterThanOrEqual(1);
    expect(t.obstacles.length).toBeLessThanOrEqual(6);
    expect(t.obstacles.every((o) => ["pine", "rock"].includes(o.kind))).toBe(true);
  });

  it("maps trail miles onto the five terrain zones in order", () => {
    expect(terrainForMiles(0)).toBe("eastForest");
    expect(terrainForMiles(500)).toBe("plains");
    expect(terrainForMiles(1000)).toBe("mountains");
    expect(terrainForMiles(1500)).toBe("desert");
    expect(terrainForMiles(2000)).toBe("westForest");
  });

  it("walks while an arrow is held and stops on release, keeping the facing", () => {
    let t = tripKey(quiet(), "ArrowRight");
    // Facing turns at once; the walk starts once the hold outlasts a tap.
    expect(t.hunter.fx).toBe(1);
    t = run(t, 0.15);
    expect(t.hunter.walking).toBe(true);
    t = run(t, 0.35);
    expect(t.hunter.x).toBeGreaterThan(COLS / 2 + 4);
    t = tripKeyUp(t, "ArrowRight");
    const x = t.hunter.x;
    t = run(t, 0.5);
    expect(t.hunter.x).toBe(x);
    expect(t.hunter.fx).toBe(1);
    expect(t.hunter.walking).toBe(false);
  });

  it("aims diagonally when two arrows are held", () => {
    const t = tripKey(tripKey(quiet(), "ArrowUp"), "ArrowRight");
    expect([t.hunter.fx, t.hunter.fy]).toEqual([1, -1]);
  });

  it("refuses to walk through a tree", () => {
    const tree = { kind: "tree" as const, x: COLS / 2 + 4, y: ROWS / 2 - 3, w: 5, h: 5, solid: true };
    let t = tripKey(quiet({ obstacles: [tree] }), "ArrowRight");
    t = run(t, 1);
    expect(t.hunter.x + 2).toBeLessThanOrEqual(tree.x + 0.01);
  });

  it("spends a bullet per shot and puts it in the air", () => {
    const t = tripKey(quiet(), " ");
    expect(t.bulletsLeft).toBe(9);
    expect(t.bulletsUsed).toBe(1);
    expect(t.bullets).toHaveLength(1);
    // The re-fire cooldown stops mashing from doubling up instantly.
    expect(tripKey(t, " ").bulletsLeft).toBe(9);
  });

  it("drops an animal the bullet hits, legs in the air, and counts the meat", () => {
    let t = quiet({ animals: [deerAt(COLS / 2 + 10, ROWS / 2 - 1)] });
    t = tripKey(t, " ");
    t = run(t, 0.5);
    expect(t.animals[0].dead).toBe(true);
    expect(t.shotLbs).toBe(ANIMALS.deer.lbs);
    expect(t.kills).toEqual([{ kind: "deer", lbs: 50 }]);
    expect(t.bullets).toHaveLength(0);
    // The carcass stays on the field for the rest of the trip.
    expect(run(t, 2).animals).toHaveLength(1);
  });

  it("caps what you can carry and reports the waste", () => {
    const buffalo = { ...deerAt(COLS / 2 + 10, ROWS / 2 - 1), kind: "buffalo" as const, hp: 1 };
    let t = quiet({ animals: [buffalo] });
    t = run(tripKey(t, " "), 0.5);
    expect(t.shotLbs).toBe(350);
    const r = tripResult(t);
    expect(r.carried).toBe(100);
    expect(r.wasted).toBe(250);
    // A full bag ends the trip shortly after, so the popup can be read.
    expect(t.done).toBe(false);
    t = run(t, 1);
    expect(t.done).toBe(true);
    expect(t.doneReason).toBe("full");
  });

  it("ends when the clock runs out", () => {
    const t = run(quiet({ seconds: 2 }), 2.5);
    expect(t.done).toBe(true);
    expect(t.doneReason).toBe("time");
  });

  it("ends when the last bullet has landed", () => {
    let t = quiet({ bullets: [], bulletsLeft: 1 });
    t = run(tripKey(t, " "), 1);
    expect(t.done).toBe(true);
    expect(t.doneReason).toBe("ammo");
  });

  it("turns without walking on a tap, walks on a hold", () => {
    let t = tripKey(quiet(), "ArrowUp");
    t = run(t, 0.05);
    t = tripKeyUp(t, "ArrowUp");
    t = run(t, 0.3);
    expect([t.hunter.fx, t.hunter.fy]).toEqual([0, -1]);
    expect(t.hunter.y).toBe(ROWS / 2);
    expect(t.hunter.walking).toBe(false);
    let held = tripKey(quiet(), "ArrowUp");
    held = run(held, 0.5);
    expect(held.hunter.y).toBeLessThan(ROWS / 2 - 3);
  });

  it("queues a shot cue on fire and an empty click with no bullets", () => {
    const t = tripKey(quiet(), " ");
    expect(t.sfx.map((c) => c.name)).toContain("shot");
    const dry = tripKey(quiet({ bulletsLeft: 0 }), " ");
    expect(dry.sfx.map((c) => c.name)).toContain("empty");
    expect(dry.bulletsUsed).toBe(0);
  });

  it("a winged bear charges and mauls you, ending the trip", () => {
    const bear = { ...deerAt(COLS / 2 + 8, ROWS / 2 - 1), kind: "bear" as const, hp: 2, vx: -1 };
    let t = quiet({ animals: [bear] });
    t = run(tripKey(t, " "), 0.3);
    const b = t.animals[0];
    expect(b.hp).toBe(1);
    expect(b.charging).toBe(true);
    expect(t.sfx.map((c) => c.name)).toContain("growl");
    // It rears up first, then closes the gap and knocks the hunter flat.
    t = run(t, 2.5);
    expect(t.hunter.down > 0 || t.done).toBe(true);
    t = run(t, 1);
    expect(t.done).toBe(true);
    expect(t.doneReason).toBe("mauled");
    expect(tripResult(t).mauled).toBe(true);
    expect(t.sfx.map((c) => c.name)).toContain("bad");
  });

  it("a second round drops a charging bear", () => {
    const bear = { ...deerAt(COLS / 2 + 8, ROWS / 2 - 1), kind: "bear" as const, hp: 1, charging: true, spooked: true, vx: -1 };
    let t = quiet({ animals: [bear] });
    t = run(tripKey(t, " "), 0.4);
    expect(t.animals[0].dead).toBe(true);
    expect(t.shotLbs).toBe(100);
    expect(t.sfx.map((c) => c.name)).toContain("bigKill");
  });

  it("a deer puts its ears up before it bounds", () => {
    const deer = { ...deerAt(20, 20), timer: 0.01, vx: 8 };
    let t = quiet({ animals: [deer] });
    t = run(t, 0.1);
    expect(t.animals[0].tell).toBeGreaterThan(0);
    expect(t.animals[0].moving).toBe(false);
    expect(t.animals[0].x).toBe(20);
    t = run(t, 0.25);
    expect(t.animals[0].moving).toBe(true);
    expect(t.animals[0].x).toBeGreaterThan(20);
  });

  it("uses the zone quota for a full bag but still caps what you carry", () => {
    expect(quotaFor("desert")).toBe(40);
    expect(quotaFor("mountains")).toBe(80);
    expect(quotaFor("plains")).toBe(100);
    const deer = deerAt(COLS / 2 + 10, ROWS / 2 - 1);
    let t: HuntTrip = { ...newTrip({ terrain: "desert", bullets: 10, seconds: 30, carryCap: 100, quota: 40 }), obstacles: [], nextSpawn: 999, animals: [deer] };
    t = run(tripKey(t, " "), 1.5);
    expect(t.done).toBe(true);
    expect(t.doneReason).toBe("full");
    expect(tripResult(t).carried).toBe(50);
    // With no quota given, the cap is the quota.
    expect(newTrip({ terrain: "plains", bullets: 1, seconds: 1, carryCap: 100 }).quota).toBe(100);
  });

  it("swipes walk for a beat and taps fire", () => {
    let t = tripPointer(quiet(), { kind: "swipe", x: 0.5, y: 0.5, dir: "ArrowLeft" });
    t = run(t, 0.2);
    expect(t.hunter.x).toBeLessThan(COLS / 2 - 1);
    const x = run(t, 0.5).hunter.x;
    expect(run(t, 1).hunter.x).toBe(x);
    expect(tripPointer(quiet(), { kind: "tap", x: 0.5, y: 0.5 }).bulletsLeft).toBe(9);
  });

  it("holds the clock during the warmup but lets you walk", () => {
    let t = quiet({ warmup: 0.6 });
    t = run(tripKey(t, "ArrowLeft"), 0.3);
    expect(t.elapsed).toBe(0);
    expect(t.hunter.x).toBeLessThan(COLS / 2);
  });
});

describe("Hunting Season", () => {
  it("opens on the first trip in the eastern forest", () => {
    const s: any = G.initialState();
    expect(s.phase).toBe("trip");
    expect(s.trip.terrain).toBe("eastForest");
    expect(s.trip.bulletsLeft).toBe(40);
  });

  it("goes trip → card → next trip, carrying the ammo box along", () => {
    let s: any = G.initialState();
    s = { ...s, trip: { ...s.trip, obstacles: [], nextSpawn: 999, seconds: 1, warmup: 0 } };
    for (let i = 0; i < 90 && s.phase === "trip"; i++) s = G.update(s, 1 / 60);
    expect(s.phase).toBe("card");
    expect(s.cardTitle).toContain("TRIP 1");
    s = G.handleKey(s, "Enter");
    expect(s.phase).toBe("trip");
    expect(s.tripIndex).toBe(1);
    expect(s.trip.terrain).toBe("plains");
    expect(s.trip.bulletsLeft).toBe(40);
  });

  it("tallies a score, remembers the best, and restarts on any key", () => {
    let s: any = G.initialState();
    // Jump to the last trip with some meat already banked and a full bag.
    s = { ...s, tripIndex: 4, score: 300, trip: { ...s.trip, terrain: "westForest", shotLbs: 120, done: true, doneReason: "full", elapsed: 20, bulletsLeft: 10 } };
    s = G.update(s, 1 / 60);
    expect(s.phase).toBe("card");
    expect(s.seasonOver).toBe(true);
    s = G.handleKey(s, " ");
    expect(s.phase).toBe("over");
    // 300 banked + 100 carried + (30-20)*2 bonus + 10 bullets * 3.
    expect(s.score).toBe(300 + 100 + 20 + 30);
    expect(s.newBest).toBe(true);
    const again: any = G.handleKey(s, "x");
    expect(again.phase).toBe("trip");
    expect(again.tripIndex).toBe(0);
    expect(again.best).toBeGreaterThanOrEqual(450);
  });

  it("surfaces the trip's cues and pauses the walk", () => {
    let s: any = G.initialState();
    s = G.handleKey({ ...s, trip: { ...s.trip, warmup: 0 } }, " ");
    expect(s.sfx.map((c: any) => c.name)).toContain("shot");
    s = G.handleKey(s, "ArrowLeft");
    s = G.onPause!(s);
    expect(s.trip.held).toEqual([]);
    expect(s.trip.hunter.walking).toBe(false);
  });

  it("ends the season early when the ammo box is empty", () => {
    let s: any = G.initialState();
    s = { ...s, trip: { ...s.trip, obstacles: [], nextSpawn: 999, warmup: 0, bulletsLeft: 0, bullets: [] } };
    s = G.update(s, 1 / 60);
    expect(s.phase).toBe("card");
    expect(s.seasonOver).toBe(true);
  });
});

describe("Hunt precision", () => {
  it("aims a tap at its position, including upward shots", () => {
    const t = tripPointer(quiet(), { kind: "tap", x: 0.5, y: 0.1 });
    expect(t.bullets[0].vy).toBeLessThan(0);
    expect(t.bullets[0].vx).toBeCloseTo(0);
  });

  it("charges while still, spends focus on a shot, and drops a bear in one shot", () => {
    const bear = { ...deerAt(COLS / 2 + 10, ROWS / 2 - 1), kind: "bear" as const, hp: 2 };
    let t = run(quiet({ animals: [bear] }), 0.9);
    expect(t.focus).toBe(1);
    t = tripKey(t, " ");
    expect(t.focus).toBe(0);
    t = run(t, 0.4);
    expect(t.animals[0].dead).toBe(true);
    expect(t.precisionScore).toBeGreaterThan(0);
    expect(t.hitStreak).toBe(1);
  });

  it("walking breaks focus and missed shots break a hit streak", () => {
    let t = run(tripKey({ ...quiet(), focus: 1 }, "ArrowRight"), 0.3);
    expect(t.focus).toBe(0);
    t = run(tripKey({ ...quiet(), hitStreak: 3 }, " "), 1);
    expect(t.hitStreak).toBe(0);
  });
});

it("does not spend ammunition after the bag is already full", () => {
  const t = quiet({ endIn: 0.5, shotLbs: 100 });
  expect(tripKey(t, " ").bulletsUsed).toBe(0);
  expect(tripPointer(t, { kind: "tap", x: 0.9, y: 0.5 }).bulletsUsed).toBe(0);
});
