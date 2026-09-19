import { beforeEach, describe, expect, it } from "vitest";
import { loadPref, seedRng } from "./gameKit";
import { OxenTrailGame as G } from "./oregonTrail";

// Drive the state machine through handleKey the way the hero wrapper does.
function press(state: any, ...keys: string[]) {
  return keys.reduce((s, k) => G.handleKey(s, k), state);
}

describe("The Oxen Trail", () => {
  beforeEach(() => {
    seedRng(42);
    window.localStorage.clear();
  });

  it("asks your occupation, then opens the store with money to spend", () => {
    const s0: any = G.initialState();
    expect(s0.phase).toBe("occupation");
    const s: any = press(s0, "1");
    expect(s.phase).toBe("store");
    expect(s.storeMode).toBe("outfit");
    expect(s.spend.reduce((a: number, b: number) => a + b, 0)).toBeLessThanOrEqual(s.budget);
  });

  it("converts dollars into supplies and heads out on Enter", () => {
    const s: any = press(G.initialState(), "1", "Enter");
    expect(s.phase).toBe("trail");
    expect(s.oxen).toBeGreaterThan(0);
    expect(s.food).toBeGreaterThan(0);
    expect(s.bullets).toBeGreaterThan(0);
    // Leftover money is kept as cash.
    expect(s.cash).toBeGreaterThanOrEqual(0);
  });

  it("blocks heading out without oxen", () => {
    let s: any = press(G.initialState(), "1");
    // Zero out the oxen row (row 0) with left presses, then try to leave.
    for (let i = 0; i < 40; i++) s = G.handleKey(s, "ArrowLeft");
    s = G.handleKey(s, "Enter");
    expect(s.phase).toBe("message");
    expect(s.msgTone).toBe("bad");
    // Dismissing returns to the store, not the trail.
    expect(G.handleKey(s, "Enter").phase).toBe("store");
  });

  it("advances miles and days when you continue on the trail", () => {
    const start: any = press(G.initialState(), "1", "Enter");
    const after: any = G.handleKey(start, "1");
    expect(after.day).toBeGreaterThan(start.day);
    expect(after.miles).toBeGreaterThan(start.miles);
  });

  it("cycles pace and rations without leaving the trail", () => {
    const s: any = press(G.initialState(), "1", "Enter");
    expect(G.handleKey(s, "3").pace).toBe((s.pace + 1) % 3);
    expect(G.handleKey(s, "4").rations).toBe((s.rations + 1) % 3);
    expect(G.handleKey(s, "3").phase).toBe("trail");
  });

  it("sends you onto the hunting field with your own bullets and terrain", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const hunt: any = G.handleKey(trail, "2");
    expect(hunt.phase).toBe("hunt");
    expect(hunt.hunt.bulletsLeft).toBe(trail.bullets);
    expect(hunt.hunt.terrain).toBe("eastForest");
    // Firing spends a bullet on the field, not the wagon's tally, until you return.
    const fired: any = G.handleKey(hunt, " ");
    expect(fired.hunt.bulletsLeft).toBe(trail.bullets - 1);
    expect(fired.bullets).toBe(trail.bullets);
  });

  it("hands the bag back to the wagon when the trip ends, costing a day", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    let hunt: any = G.handleKey(trail, "2");
    // A trip is thirty seconds; run it out with no animals shot.
    for (let i = 0; i < 60 * 40 && !hunt.hunt.done; i++) hunt = G.update(hunt, 1 / 60);
    expect(hunt.hunt.done).toBe(true);
    // Left alone, the field lingers briefly and then returns on its own.
    let idle: any = hunt;
    for (let i = 0; i < 60 * 3 && idle.phase === "hunt"; i++) idle = G.update(idle, 1 / 60);
    expect(idle.phase).toBe("message");
    const back: any = G.handleKey(hunt, "Enter");
    expect(back.phase).toBe("message");
    expect(back.day).toBe(trail.day + 1);
    expect(back.food).toBeGreaterThanOrEqual(trail.food);
    expect(back.hunt).toBeNull();
  });

  it("caps a hunt's haul at the carry limit and reports the waste", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const hunt: any = G.handleKey(trail, "2");
    // Pretend the trip bagged a buffalo: 350 lb shot, 100 carried.
    const done = { ...hunt.hunt, done: true, doneReason: "full", shotLbs: 350, bulletsUsed: 3, bulletsLeft: hunt.hunt.bulletsLeft - 3, kills: [{ kind: "buffalo", lbs: 350 }] };
    const back: any = G.handleKey({ ...hunt, hunt: done }, "Enter");
    expect(back.food).toBe(trail.food + 100);
    expect(back.bullets).toBe(trail.bullets - 3);
    expect(back.msgLines.join(" ")).toContain("250 lbs wasted");
  });

  it("refuses to hunt with no bullets", () => {
    const trail: any = { ...press(G.initialState(), "1", "Enter"), bullets: 0 };
    const refused: any = G.handleKey(trail, "2");
    expect(refused.phase).toBe("message");
    expect(refused.msgTone).toBe("bad");
  });

  it("reaches Oregon and reports a score when the trail is done", () => {
    let s: any = press(G.initialState(), "3", "Enter");
    // Continue (dismissing any message screens, taking option 2 on decisions)
    // until the journey ends.
    for (let i = 0; i < 400 && s.phase !== "over"; i++) {
      s = s.phase === "trail" ? G.handleKey(s, "1") : G.handleKey(s, s.phase === "river" ? (s.food >= s.party.filter((m: any) => m.alive).length * 6 ? "3" : "2") : s.phase === "choice" ? "2" : "Enter");
    }
    expect(s.phase).toBe("over");
    expect(typeof s.score).toBe("number");
    // Either you made it or the party perished — both are terminal with a score.
    expect(s.arrived || s.cause.length > 0).toBe(true);
  });

  it("restarts a fresh outfit from the game-over screen, keeping the best", () => {
    const over: any = { ...G.initialState(), phase: "over", arrived: true, score: 999, best: 999 };
    const again: any = G.handleKey(over, "Enter");
    expect(again.phase).toBe("occupation");
    expect(again.miles).toBe(0);
    expect(again.best).toBe(999);
  });

  it("poses two-way decisions and resolves them by key", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const card: any = { ...trail, phase: "choice", choiceId: "trader", choiceOptions: ["Trade", "Decline"], msgTitle: "A trader hails you", msgLines: [] };
    const traded: any = G.handleKey(card, "1");
    expect(traded.phase).toBe("message");
    expect(traded.food).toBe(trail.food + 100);
    expect(traded.bullets).toBe(trail.bullets - 40);
    const declined: any = G.handleKey(card, "2");
    expect(declined.food).toBe(trail.food);
    expect(G.handleKey(declined, "Enter").phase).toBe("trail");
  });

  it("offers a paid ferry at rivers that is always safe", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const river: any = { ...trail, phase: "river", riverName: "Green River", cash: 50, nextLandmark: 5 };
    const crossed: any = G.handleKey(river, "4");
    expect(crossed.phase).toBe("message");
    expect(crossed.msgTone).toBe("good");
    expect(crossed.cash).toBe(30);
    // Broke travellers can't buy passage.
    expect(G.handleKey({ ...river, cash: 5 }, "4").phase).toBe("river");
  });

  it("scores the farmer triple", () => {
    const base: any = { ...press(G.initialState(), "1", "Enter"), phase: "trail" };
    const banker: any = G.handleKey({ ...base, occupation: 0, miles: 2039, nextLandmark: 9 }, "1");
    const farmer: any = G.handleKey({ ...base, occupation: 2, miles: 2039, nextLandmark: 9 }, "1");
    expect(banker.arrived && farmer.arrived).toBe(true);
    // Same wagon, same day count: the farmer's multiplier is the only difference.
    expect(farmer.score).toBeGreaterThan(banker.score * 2);
  });

  it("gives every traveler their own health and names the one who dies", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    expect(trail.party.every((m: any) => m.health === 100)).toBe(true);
    // Sarah is at death's door with dysentery; one hard leg finishes her.
    const party = trail.party.map((m: any) => (m.name === "Sarah" ? { ...m, health: 3, ailment: "dysentery" } : m));
    let s: any = { ...trail, party, pace: 2, rations: 2, food: 0, nextLandmark: 9, miles: 100 };
    s = G.handleKey(s, "1");
    expect(s.phase).toBe("message");
    expect(s.msgTitle).toBe("Sarah has died");
    expect(s.msgLines[0]).toBe("of dysentery.");
    expect(s.party.find((m: any) => m.name === "Sarah").alive).toBe(false);
    // The party average now only counts the living.
    expect(s.health).toBe(Math.round(s.party.filter((m: any) => m.alive).reduce((a: number, m: any) => a + m.health, 0) / 4));
    // The grave is remembered for the attract screen.
    expect(JSON.parse(loadPref("trail-tomb")!)).toMatchObject({ name: "Sarah", cause: "dysentery" });
  });

  it("puts the whole wagon on the tombstone when the last traveler goes", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const party = trail.party.map((m: any, i: number) => (i === 0 ? { ...m, health: 2, ailment: "cholera" } : { ...m, alive: false, health: 0 }));
    const over: any = G.handleKey({ ...trail, party, pace: 2, rations: 2, food: 0, nextLandmark: 9, miles: 100 }, "1");
    expect(over.phase).toBe("over");
    expect(over.epitaph).toBe("Wagon Boss");
    expect(over.cause).toBe("cholera");
    expect(over.sfx.some((e: any) => e.name === "bad")).toBe(true);
  });

  it("halves what bandits take once the dog is fed", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const card: any = { ...trail, phase: "choice", choiceId: "dog", choiceOptions: ["Feed it", "Shoo it off"], food: 500 };
    const fed: any = G.handleKey(card, "1");
    expect(fed.flags.dog).toBe(true);
    expect(fed.food).toBe(485);
    // Same seed, same raid: with the dog the losses are half.
    seedRng(7);
    let plain: any = { ...trail, food: 500, bullets: 200, flags: {} };
    let guarded: any = { ...trail, food: 500, bullets: 200, flags: { dog: true } };
    // Drive the bandit event directly through the same seeded rolls.
    const bandits = (st: any) => {
      // Search legs until "Bandits attack!" comes up, then measure the loss.
      let cur = st;
      for (let i = 0; i < 300; i++) {
        seedRng(1000 + i);
        const next = G.handleKey({ ...cur, miles: 100, nextLandmark: 9 }, "1");
        if (next.phase === "message" && next.msgTitle === "Bandits attack!") return { food: cur.food - next.food + Math.min(cur.food, 55 * 5), title: next.msgLines[0] };
      }
      throw new Error("no bandit raid in 300 tries");
    };
    const a = bandits(plain);
    const b = bandits(guarded);
    expect(b.title).toContain("dog raised the alarm");
    expect(b.food).toBeLessThan(a.food);
  });

  it("brings the trader back with a worse deal only after you refused him", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const card: any = { ...trail, phase: "choice", choiceId: "trader", choiceOptions: ["Trade", "Decline"] };
    const refused: any = G.handleKey(card, "2");
    expect(refused.flags.refusedTrader).toBe(true);
    const back: any = { ...refused, phase: "choice", choiceId: "traderBack", choiceOptions: ["Take it", "Refuse again"], bullets: 100 };
    const taken: any = G.handleKey(back, "1");
    expect(taken.food).toBe(refused.food + 100);
    expect(taken.bullets).toBe(40);
    expect(taken.flags.traderBack).toBe(true);
    // Without refusing first, the return visit never rolls.
    const fresh: any = { ...trail, flags: {}, bullets: 100 };
    for (let i = 0; i < 200; i++) {
      seedRng(i);
      const next = G.handleKey({ ...fresh, miles: 100, nextLandmark: 9 }, "1");
      expect(next.choiceId === "traderBack" && next.phase === "choice").toBe(false);
    }
  });

  it("summarises the last leg on the trail screen", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    expect(trail.lastLeg).toBeNull();
    let s: any = G.handleKey({ ...trail, miles: 100, nextLandmark: 9 }, "1");
    while (s.phase !== "trail" && s.phase !== "over") s = G.handleKey(s, s.phase === "choice" ? "2" : "Enter");
    expect(s.lastLeg).not.toBeNull();
    expect(s.lastLeg.miles).toBeGreaterThan(0);
    expect(s.lastLeg.ate).toBe(5 * 55);
    expect(typeof s.lastLeg.dh).toBe("number");
  });

  it("enters your run on the all-time table, capped at five", () => {
    const base: any = { ...press(G.initialState(), "1", "Enter"), phase: "trail", miles: 2039, nextLandmark: 9, cash: 5000 };
    const done: any = G.handleKey(base, "1");
    expect(done.arrived).toBe(true);
    expect(done.rank).toBe(1);
    const table = JSON.parse(loadPref("trail-scores")!);
    expect(table.length).toBe(5);
    expect(table[0]).toMatchObject({ name: "You", score: done.score, occupation: "Banker" });
    // A poor run doesn't place.
    const poor: any = G.handleKey({ ...base, cash: 0, food: 0, party: base.party.map((m: any, i: number) => (i ? { ...m, alive: false } : m)) }, "1");
    expect(poor.rank).toBe(0);
    expect(JSON.parse(loadPref("trail-scores")!).length).toBe(5);
  });

  it("picks a decision by tapping its half of the card", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const card: any = { ...trail, phase: "choice", choiceId: "trader", choiceOptions: ["Trade", "Decline"], msgTitle: "", msgLines: [] };
    const left: any = G.handlePointer!(card, { kind: "tap", x: 0.2, y: 0.7 });
    expect(left.food).toBe(trail.food + 100);
    const right: any = G.handlePointer!(card, { kind: "tap", x: 0.8, y: 0.7 });
    expect(right.food).toBe(trail.food);
    // A tap on the trail menu picks the row under it: row 3 cycles pace.
    const paced: any = G.handlePointer!(trail, { kind: "tap", x: 0.7, y: 98 / 136 });
    expect(paced.pace).toBe(1);
  });

  it("wounds a traveler when the bear wins the hunt", () => {
    const trail: any = press(G.initialState(), "1", "Enter");
    const hunt: any = G.handleKey(trail, "2");
    const done = { ...hunt.hunt, done: true, doneReason: "mauled", shotLbs: 50, bulletsUsed: 4, bulletsLeft: hunt.hunt.bulletsLeft - 4, kills: [{ kind: "deer", lbs: 50 }] };
    const back: any = G.handleKey({ ...hunt, hunt: done }, "Enter");
    expect(back.phase).toBe("message");
    expect(back.msgTitle).toBe("A bear got the better of you");
    const hurt = back.party.find((m: any) => m.ailment === "mauling");
    expect(hurt.health).toBe(80);
    expect(back.food).toBe(trail.food + 50);
  });

  it("clicks on menu presses and chimes on good news", () => {
    const s0: any = G.initialState();
    const s1: any = G.handleKey(s0, "1");
    expect(s1.sfx.at(-1).name).toBe("menu");
    const trail: any = press(s1, "Enter");
    const win: any = G.handleKey({ ...trail, miles: 2039, nextLandmark: 9 }, "1");
    expect(win.sfx.some((e: any) => e.name === "good" || e.name === "best")).toBe(true);
  });
});

describe("Trail decisions", () => {
  it("rests at camp for food and days, healing only living travelers", () => {
    const trail = press(G.initialState(), "1", "Enter");
    const party = trail.party.map((m: any, i: number) => ({ ...m, health: i ? 40 : 0, alive: i > 0 }));
    const camp = press({ ...trail, party, health: 40 }, "6");
    expect(camp.phase).toBe("camp");
    const rested = press(camp, "1");
    expect(rested.day).toBe(trail.day + 2);
    expect(rested.food).toBe(trail.food - 24);
    expect(rested.party[1].health).toBe(54);
    expect(rested.party[0].alive).toBe(false);
    expect(rested.party[0].health).toBe(0);
  });

  it("allows one forage and scout per leg, with explicit resource costs", () => {
    const trail = press(G.initialState(), "1", "Enter");
    let s = press(trail, "6", "2", "Enter", "6", "2");
    expect(s.food).toBe(trail.food + 45);
    expect(s.day).toBe(trail.day + 1);
    s = press({ ...s, phase: "camp" }, "3");
    expect(s.scouted).toBe(true);
    expect(s.food).toBe(trail.food + 35);
    seedRng(42);
    const ordinary = press({ ...trail, nextLandmark: 9 }, "1");
    seedRng(42);
    const scouted = press({ ...trail, nextLandmark: 9, scouted: true, foraged: true }, "1");
    expect(scouted.miles).toBe(ordinary.miles + 25);
    expect(scouted.scouted).toBe(false);
    expect(scouted.foraged).toBe(false);
  });

  it("keeps the fort after Green River on the route", () => {
    const trail = press(G.initialState(), "1", "Enter");
    const river = press({ ...trail, miles: 1000, nextLandmark: 5, food: 2000 }, "1");
    expect(river.phase).toBe("river");
    expect(river.nextLandmark).toBe(6);
    const crossed = press(river, "3", "Enter");
    expect(crossed.nextLandmark).toBe(6);
    expect(crossed.day).toBe(river.day + 2);
    expect(crossed.food).toBe(river.food - 30);
  });

  it("refuses recovery without the supplies instead of healing for free", () => {
    const trail = press(G.initialState(), "1", "Enter");
    const result = press({ ...trail, food: 0, health: 40 }, "6", "1");
    expect(result.day).toBe(trail.day);
    expect(result.health).toBe(40);
    expect(result.msgTitle).toBe("Not enough provisions");
  });
});
