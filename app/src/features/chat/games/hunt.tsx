// Hunting Season — a remake of the hunting activity from the 1985 Apple II
// Oregon Trail. You're dropped into an overhead patch of the countryside with a
// rifle, a handful of bullets, and a clock. Animals wander across the field;
// you walk in eight directions, aim the way you face, and fire. Shot animals
// flip upside-down (legs in the air — the quirk every kid remembers) and stay
// where they fell. The catch, straight from the original: however much you
// shoot, you can only carry 100 lb back to the wagon. Overshoot and the summary
// tells you what you wasted.
//
// The module is split in two so the Oxen Trail sim can embed a single hunting
// trip: `newTrip/tripKey/tripKeyUp/tripUpdate/tripResult/HuntScene` run one
// trip as pure state; `HuntGame` wraps five trips (one per terrain zone along
// the trail) into a scored "season" for the arcade cabinet.

import type { ThemePalette } from "../../../lib/types";
import {
  burst,
  clamp,
  COLS,
  GameFrame,
  loadBest,
  motionScale,
  Particles,
  pickRand,
  poly,
  Popups,
  pushSfx,
  Px,
  PxText,
  rand,
  randInt,
  ROWS,
  saveBest,
  sceneColors,
  shakeTransform,
  stepParticles,
  stepPopups,
  U,
  type HeroGameDefinition,
  type Particle,
  type PointerInput,
  type Popup,
  type SceneColors,
  type SfxEvent,
} from "./gameKit";

// ---- terrain ----------------------------------------------------------------

export type Terrain = "eastForest" | "plains" | "mountains" | "desert" | "westForest";

export const TERRAIN_ORDER: Terrain[] = ["eastForest", "plains", "mountains", "desert", "westForest"];

export const TERRAIN_NAMES: Record<Terrain, string> = {
  eastForest: "Eastern Forest",
  plains: "Plains",
  mountains: "Rocky Mountains",
  desert: "Desert",
  westForest: "Western Forest",
};

/** How much meat counts as a "full bag" in a zone. The desert is small-game
    country, so its bar is lower — otherwise it's a dead trip by design. */
export function quotaFor(terrain: Terrain): number {
  if (terrain === "desert") return 40;
  if (terrain === "mountains") return 80;
  return 100;
}

/** Which zone of the trail you're hunting in, by miles travelled (0–2040). */
export function terrainForMiles(miles: number): Terrain {
  if (miles < 250) return "eastForest";
  if (miles < 930) return "plains";
  if (miles < 1300) return "mountains";
  if (miles < 1700) return "desert";
  return "westForest";
}

type ObstacleKind = "tree" | "pine" | "rock" | "cactus" | "shrub" | "tuft";

interface Obstacle {
  kind: ObstacleKind;
  x: number;
  y: number;
  w: number;
  h: number;
  /** Solid objects block both walking and bullets; the rest is set dressing. */
  solid: boolean;
}

const OBSTACLE_SIZE: Record<ObstacleKind, [number, number, boolean]> = {
  tree: [5, 5, true],
  pine: [3, 5, true],
  rock: [3, 2, true],
  cactus: [2, 4, true],
  shrub: [3, 2, false],
  tuft: [2, 1, false],
};

// The original picked 4–6 objects per hunt from the zone's own flora.
const TERRAIN_OBJECTS: Record<Terrain, ObstacleKind[]> = {
  eastForest: ["tree", "tree", "tuft", "tuft"],
  plains: ["tuft", "tuft", "tuft", "rock"],
  mountains: ["pine", "pine", "rock", "rock"],
  desert: ["cactus", "cactus", "shrub", "rock"],
  westForest: ["pine", "pine", "pine", "tuft"],
};

// ---- animals ----------------------------------------------------------------

export type AnimalKind = "squirrel" | "rabbit" | "deer" | "bear" | "buffalo";

interface AnimalSpec {
  lbs: number;
  speed: number; // cells per second
  w: number;
  h: number;
  hp: number;
  /** How the animal moves: constant, jittery, zigzag, or burst-and-pause. */
  gait: "steady" | "jitter" | "zigzag" | "burst";
}

export const ANIMALS: Record<AnimalKind, AnimalSpec> = {
  squirrel: { lbs: 2, speed: 16, w: 2, h: 2, hp: 1, gait: "jitter" },
  rabbit: { lbs: 4, speed: 14, w: 2, h: 2, hp: 1, gait: "zigzag" },
  deer: { lbs: 50, speed: 11, w: 4, h: 3, hp: 1, gait: "burst" },
  bear: { lbs: 100, speed: 5, w: 5, h: 3, hp: 2, gait: "steady" },
  buffalo: { lbs: 350, speed: 4, w: 6, h: 3, hp: 2, gait: "steady" },
};

// Spawn weights per zone: the plains are buffalo country, the forests deer.
const TERRAIN_MIX: Record<Terrain, [AnimalKind, number][]> = {
  eastForest: [["deer", 4], ["squirrel", 3], ["bear", 2], ["rabbit", 2]],
  plains: [["buffalo", 4], ["rabbit", 3], ["deer", 2], ["squirrel", 1]],
  mountains: [["bear", 3], ["deer", 4], ["squirrel", 2], ["rabbit", 1]],
  desert: [["rabbit", 4], ["squirrel", 3], ["deer", 2], ["bear", 0.5]],
  westForest: [["deer", 4], ["squirrel", 2], ["bear", 2], ["rabbit", 1]],
};

interface Animal {
  id: number;
  kind: AnimalKind;
  x: number; // top-left of the bounding box, in cells
  y: number;
  vx: number;
  vy: number;
  hp: number;
  dead: boolean;
  /** Seconds left standing still (burst gait), or until the next zig. */
  timer: number;
  moving: boolean;
  age: number;
  /** Set once hit but not dropped: the animal bolts. */
  spooked: boolean;
  /** Seconds of telegraph before a deer bounds (ears up). */
  tell: number;
  /** A winged bear comes for you. `rear` is its rear-up before the charge,
      `retarget` the countdown to re-aiming at the hunter. */
  charging: boolean;
  rear: number;
  retarget: number;
}

interface Bullet {
  id: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
}

interface Hunter {
  x: number; // center, in cells
  y: number;
  fx: number; // facing vector, each -1 | 0 | 1
  fy: number;
  walking: boolean;
  recoil: number;
  flash: number;
  /** Seconds left lying flat after a bear got you. */
  down: number;
}

// ---- one hunting trip ---------------------------------------------------------

export interface HuntTripOptions {
  terrain: Terrain;
  bullets: number;
  seconds: number;
  carryCap: number;
  /** Soft-start seconds: you can walk but the clock and spawns hold. */
  warmup?: number;
  /** Meat that counts as a full bag (defaults to the carry cap). */
  quota?: number;
}

export type DoneReason = "" | "time" | "ammo" | "full" | "mauled";

export interface HuntTrip {
  terrain: Terrain;
  hunter: Hunter;
  held: string[];
  animals: Animal[];
  bullets: Bullet[];
  obstacles: Obstacle[];
  seconds: number;
  elapsed: number;
  warmup: number;
  bulletsLeft: number;
  bulletsUsed: number;
  carryCap: number;
  shotLbs: number;
  kills: { kind: AnimalKind; lbs: number }[];
  particles: Particle[];
  popups: Popup[];
  hitStop: number;
  shake: number;
  fireCooldown: number;
  nextSpawn: number;
  /** Countdown to ending the trip once the bag is full (lets the popup land). */
  endIn: number;
  done: boolean;
  doneReason: DoneReason;
  clock: number;
  /** Full-bag threshold; `carryCap` still caps what comes home. */
  quota: number;
  /** Sound cues for the wrapper (see pushSfx). */
  sfx: SfxEvent[];
  /** Seconds each arrow has been down: a tap turns, a hold walks. */
  heldFor: Record<string, number>;
  /** A swipe walks you this way until `until` (trip clock). */
  swipeWalk: { dx: number; dy: number; until: number } | null;
  /** Last whole second announced by the countdown tick. */
  tickAt: number;
  /** Countdown from the mauling to the trip's end. */
  mauledIn: number;
}

// Playable field: HUD rows on top, the wrapper's help line along the bottom.
const FIELD_TOP = 4;
const FIELD_BOTTOM = ROWS - 3;
const HUNTER_SPEED = 14;
const BULLET_SPEED = COLS * 1.5;
const FIRE_COOLDOWN = 0.18;
const MAX_IN_FLIGHT = 2;
const MAX_ANIMALS = 3;

const ARROWS = ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"];
const TAP_TURN = 0.12; // hold an arrow longer than this to walk; shorter just turns
const SWIPE_WALK = 0.35; // seconds a swipe keeps you walking
const CHARGE_SPEED = 1.4; // × bear speed once it comes for you
const REAR_UP = 0.3;
const KNOCKDOWN = 0.6;
let nextId = 1;
const ri = randInt;
const pick = pickRand;

function hunterRect(h: Hunter) {
  return { x: h.x - 1, y: h.y - 2, w: 3, h: 4 };
}

function overlaps(a: { x: number; y: number; w: number; h: number }, b: { x: number; y: number; w: number; h: number }) {
  return a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
}

function inside(px: number, py: number, r: { x: number; y: number; w: number; h: number }) {
  return px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h;
}

function scatterObstacles(terrain: Terrain): Obstacle[] {
  const out: Obstacle[] = [];
  const want = ri(4, 6);
  const keepClear = { x: COLS / 2 - 6, y: ROWS / 2 - 5, w: 12, h: 10 };
  for (let tries = 0; tries < 60 && out.length < want; tries++) {
    const kind = pick(TERRAIN_OBJECTS[terrain]);
    const [w, h, solid] = OBSTACLE_SIZE[kind];
    const ob = { kind, x: ri(1, COLS - w - 2), y: ri(FIELD_TOP + 1, FIELD_BOTTOM - h - 1), w, h, solid };
    if (overlaps(ob, keepClear)) continue;
    if (out.some((o) => overlaps({ x: ob.x - 1, y: ob.y - 1, w: w + 2, h: h + 2 }, o))) continue;
    out.push(ob);
  }
  return out;
}

export function newTrip(opts: HuntTripOptions): HuntTrip {
  return {
    terrain: opts.terrain,
    hunter: { x: COLS / 2, y: ROWS / 2, fx: 1, fy: 0, walking: false, recoil: 0, flash: 0, down: 0 },
    held: [],
    animals: [],
    bullets: [],
    obstacles: scatterObstacles(opts.terrain),
    seconds: opts.seconds,
    elapsed: 0,
    warmup: opts.warmup ?? 0,
    bulletsLeft: opts.bullets,
    bulletsUsed: 0,
    carryCap: opts.carryCap,
    shotLbs: 0,
    kills: [],
    particles: [],
    popups: [],
    hitStop: 0,
    shake: 0,
    fireCooldown: 0,
    nextSpawn: 0.6,
    endIn: 0,
    done: false,
    doneReason: "",
    clock: 0,
    quota: opts.quota ?? opts.carryCap,
    sfx: [],
    heldFor: {},
    swipeWalk: null,
    tickAt: 0,
    mauledIn: 0,
  };
}

function heldVector(held: string[]): [number, number] {
  const dx = (held.includes("ArrowRight") ? 1 : 0) - (held.includes("ArrowLeft") ? 1 : 0);
  const dy = (held.includes("ArrowDown") ? 1 : 0) - (held.includes("ArrowUp") ? 1 : 0);
  return [dx, dy];
}

function fire(t: HuntTrip): HuntTrip {
  if (t.hunter.down > 0) return t;
  if (t.bulletsLeft <= 0) return { ...t, sfx: pushSfx(t.sfx, "empty") };
  if (t.fireCooldown > 0 || t.bullets.length >= MAX_IN_FLIGHT) return t;
  const h = t.hunter;
  const len = Math.hypot(h.fx, h.fy) || 1;
  const vx = (h.fx / len) * BULLET_SPEED;
  const vy = (h.fy / len) * BULLET_SPEED;
  const bullet: Bullet = { id: nextId++, x: h.x + h.fx * 2, y: h.y + h.fy * 2, vx, vy };
  return {
    ...t,
    bullets: [...t.bullets, bullet],
    bulletsLeft: t.bulletsLeft - 1,
    bulletsUsed: t.bulletsUsed + 1,
    fireCooldown: FIRE_COOLDOWN,
    hunter: { ...h, flash: 0.08, recoil: 0.1 },
    sfx: pushSfx(t.sfx, "shot"),
  };
}

export function tripKey(t: HuntTrip, key: string): HuntTrip {
  if (t.done) return t;
  if (key === " " || key === "Enter") return fire(t);
  if (!ARROWS.includes(key)) return t;
  if (t.held.includes(key)) return t; // key repeat
  const held = [...t.held, key];
  const [dx, dy] = heldVector(held);
  // Facing follows the held combination immediately (so two arrows aim
  // diagonally); walking waits until the hold outlasts a tap (see tripUpdate).
  // If the combination cancels out (← and →), keep the last facing.
  const hunter = dx || dy ? { ...t.hunter, fx: dx, fy: dy } : t.hunter;
  return { ...t, held, heldFor: { ...t.heldFor, [key]: 0 }, hunter };
}

export function tripKeyUp(t: HuntTrip, key: string): HuntTrip {
  if (!ARROWS.includes(key) || !t.held.includes(key)) return t;
  const held = t.held.filter((k) => k !== key);
  const heldFor = { ...t.heldFor };
  delete heldFor[key];
  const [dx, dy] = heldVector(held);
  // Letting go stops the walk but leaves you aiming where you were.
  const hunter = dx || dy ? { ...t.hunter, fx: dx, fy: dy } : { ...t.hunter, walking: false };
  return { ...t, held, heldFor, hunter };
}

/** Taps fire; swipes walk that way for a beat (trackpad play). */
export function tripPointer(t: HuntTrip, p: PointerInput): HuntTrip {
  if (t.done) return t;
  if (p.kind === "tap" || !p.dir) return fire(t);
  const dx = p.dir === "ArrowLeft" ? -1 : p.dir === "ArrowRight" ? 1 : 0;
  const dy = p.dir === "ArrowUp" ? -1 : p.dir === "ArrowDown" ? 1 : 0;
  return { ...t, swipeWalk: { dx, dy, until: t.clock + SWIPE_WALK }, hunter: { ...t.hunter, fx: dx, fy: dy } };
}

/** The movement vector this frame: arrows held past a tap, or a live swipe. */
function moveVector(t: HuntTrip, clock: number): [number, number] {
  if (t.swipeWalk && clock < t.swipeWalk.until) return [t.swipeWalk.dx, t.swipeWalk.dy];
  const walking = t.held.filter((k) => (t.heldFor[k] ?? 0) >= TAP_TURN);
  return heldVector(walking);
}

function weightedKind(terrain: Terrain): AnimalKind {
  const mix = TERRAIN_MIX[terrain];
  const total = mix.reduce((s, [, w]) => s + w, 0);
  let r = rand() * total;
  for (const [kind, w] of mix) {
    r -= w;
    if (r <= 0) return kind;
  }
  return mix[0][0];
}

function spawnAnimal(terrain: Terrain): Animal {
  const kind = weightedKind(terrain);
  const spec = ANIMALS[kind];
  // Small game sometimes darts in from the top or bottom; big game always
  // crosses side to side so you get a long look at it.
  const vertical = spec.w <= 4 && rand() < 0.25;
  let x: number, y: number, vx: number, vy: number;
  if (vertical) {
    const fromTop = rand() < 0.5;
    x = ri(4, COLS - spec.w - 4);
    y = fromTop ? FIELD_TOP - spec.h - 2 : FIELD_BOTTOM + 2;
    vy = (fromTop ? 1 : -1) * spec.speed;
    vx = (rand() - 0.5) * spec.speed * 0.4;
  } else {
    const fromLeft = rand() < 0.5;
    x = fromLeft ? -spec.w - 2 : COLS + 2;
    y = ri(FIELD_TOP + 2, FIELD_BOTTOM - spec.h - 2);
    vx = (fromLeft ? 1 : -1) * spec.speed;
    vy = (rand() - 0.5) * spec.speed * 0.3;
  }
  return { id: nextId++, kind, x, y, vx, vy, hp: spec.hp, dead: false, timer: 0.4 + rand() * 0.6, moving: true, age: 0, spooked: false, tell: 0, charging: false, rear: 0, retarget: 0 };
}

function stepAnimal(a: Animal, dt: number, hunter: Hunter): Animal {
  if (a.dead) return a;
  const spec = ANIMALS[a.kind];
  let { vx, vy, timer, moving } = a;
  let tell = a.tell ?? 0;
  let rear = a.rear ?? 0;
  let retarget = a.retarget ?? 0;
  timer -= dt;

  // A winged bear rears up, then comes straight for you, re-aiming as you move.
  if (a.charging) {
    if (rear > 0) {
      rear -= dt;
      return { ...a, rear, moving: false, age: a.age + dt };
    }
    retarget -= dt;
    if (retarget <= 0) {
      const cx = a.x + spec.w / 2;
      const cy = a.y + spec.h / 2;
      const ddx = hunter.x - cx;
      const ddy = hunter.y - cy;
      const len = Math.hypot(ddx, ddy) || 1;
      vx = (ddx / len) * spec.speed * CHARGE_SPEED;
      vy = (ddy / len) * spec.speed * CHARGE_SPEED;
      retarget = 0.4;
    }
    return { ...a, x: a.x + vx * dt, y: a.y + vy * dt, vx, vy, timer, moving: true, retarget, rear: 0, age: a.age + dt };
  }

  const boost = a.spooked ? 1.6 : 1;
  switch (spec.gait) {
    case "burst":
      // Deer bound, freeze, ears up, bound — the pause is your window and the
      // ears are your warning that it's about to close.
      if (a.spooked) {
        moving = true;
        tell = 0;
      } else if (moving) {
        if (timer <= 0) {
          moving = false;
          timer = 0.35 + rand() * 0.4;
        }
      } else if (tell > 0) {
        tell -= dt;
        if (tell <= 0) {
          tell = 0;
          moving = true;
          timer = 0.6 + rand() * 0.6;
        }
      } else if (timer <= 0) {
        tell = 0.2;
      }
      break;
    case "zigzag":
      if (timer <= 0) {
        vy = (rand() < 0.5 ? -1 : 1) * spec.speed * 0.5;
        timer = 0.3 + rand() * 0.4;
      }
      break;
    case "jitter":
      if (timer <= 0) {
        vy = (rand() - 0.5) * spec.speed;
        vx = Math.sign(vx) * spec.speed * (0.7 + rand() * 0.6);
        timer = 0.15 + rand() * 0.25;
      }
      break;
    default:
      break;
  }
  const mv = moving ? boost : 0;
  let y = a.y + vy * mv * dt;
  // Keep side-to-side travellers inside the field; edge-bound ones pass through.
  if (Math.abs(vx) > Math.abs(vy)) {
    if (y < FIELD_TOP) {
      y = FIELD_TOP;
      vy = Math.abs(vy);
    } else if (y + spec.h > FIELD_BOTTOM) {
      y = FIELD_BOTTOM - spec.h;
      vy = -Math.abs(vy);
    }
  }
  return { ...a, x: a.x + vx * mv * dt, y, vx, vy, timer, moving, tell, rear, retarget, age: a.age + dt };
}

function offField(a: Animal) {
  const spec = ANIMALS[a.kind];
  return a.x + spec.w < -6 || a.x > COLS + 6 || a.y + spec.h < FIELD_TOP - 6 || a.y > FIELD_BOTTOM + 6;
}

function tryMove(t: HuntTrip, x: number, y: number): boolean {
  const r = hunterRect({ ...t.hunter, x, y });
  return !t.obstacles.some((o) => o.solid && overlaps(r, o));
}

export function tripUpdate(t: HuntTrip, dt: number): HuntTrip {
  if (t.done) return t;
  const clock = t.clock + dt;
  // Hit-stop: the whole world holds for a beat when something drops.
  if (t.hitStop > 0) return { ...t, clock, hitStop: t.hitStop - dt, shake: Math.max(0, t.shake - dt) };

  // Count the holds: an arrow down past a tap starts you walking.
  let heldFor = t.heldFor;
  if (t.held.length) {
    heldFor = { ...heldFor };
    for (const k of t.held) heldFor[k] = (heldFor[k] ?? 0) + dt;
  }
  const swipeWalk = t.swipeWalk && clock < t.swipeWalk.until ? t.swipeWalk : null;
  const [mx, my] = moveVector({ ...t, heldFor, swipeWalk }, clock);
  let sfx = t.sfx;

  // Walk. Axis-separated so brushing a tree slides you along it.
  let hunter = { ...t.hunter, recoil: Math.max(0, t.hunter.recoil - dt), flash: Math.max(0, t.hunter.flash - dt), down: Math.max(0, t.hunter.down - dt) };
  hunter.walking = (mx !== 0 || my !== 0) && hunter.down <= 0;
  if (hunter.walking) {
    const norm = mx && my ? Math.SQRT1_2 : 1;
    const nx = clamp(hunter.x + mx * norm * HUNTER_SPEED * dt, 2, COLS - 3);
    const ny = clamp(hunter.y + my * norm * HUNTER_SPEED * dt, FIELD_TOP + 2, FIELD_BOTTOM - 2);
    if (tryMove(t, nx, ny)) hunter = { ...hunter, x: nx, y: ny };
    else if (tryMove(t, nx, hunter.y)) hunter = { ...hunter, x: nx };
    else if (tryMove(t, hunter.x, ny)) hunter = { ...hunter, y: ny };
  }

  let particles = stepParticles(t.particles, dt, 20);
  let popups = stepPopups(t.popups, dt);
  const shake = Math.max(0, t.shake - dt);
  const fireCooldown = Math.max(0, t.fireCooldown - dt);

  // Soft start: you can get your bearings, but nothing else moves yet.
  if (t.warmup > 0) {
    return { ...t, clock, hunter, heldFor, swipeWalk, particles, popups, shake, fireCooldown, warmup: t.warmup - dt };
  }

  // The bear got you: lie there for a beat, then the trip is over.
  if (t.mauledIn > 0) {
    const mauledIn = t.mauledIn - dt;
    const animals = t.animals.map((a) => (a.charging ? { ...a, charging: false, moving: false } : a));
    if (mauledIn <= 0) return { ...t, clock, hunter, heldFor, swipeWalk, animals, particles, popups, shake, fireCooldown, mauledIn: 0, done: true, doneReason: "mauled" };
    return { ...t, clock, hunter, heldFor, swipeWalk, animals, particles, popups, shake, fireCooldown, mauledIn };
  }

  const elapsed = t.elapsed + dt;
  let animals = t.animals.map((a) => stepAnimal(a, dt, hunter)).filter((a) => a.dead || !offField(a));
  let nextSpawn = t.nextSpawn - dt;
  const live = animals.filter((a) => !a.dead).length;
  if (nextSpawn <= 0 && live < MAX_ANIMALS && elapsed < t.seconds - 1.5) {
    animals = [...animals, spawnAnimal(t.terrain)];
    nextSpawn = live === 0 ? 0.8 + rand() * 0.8 : 1.2 + rand() * 1.6;
  }

  // Bullets fly in sub-steps of one cell so a fast round can't tunnel through
  // a squirrel.
  let shotLbs = t.shotLbs;
  let kills = t.kills;
  let hitStop = 0;
  let newShake = shake;
  let endIn = t.endIn;
  const bullets: Bullet[] = [];
  for (const b of t.bullets) {
    const steps = Math.max(1, Math.ceil(Math.hypot(b.vx, b.vy) * dt));
    let x = b.x;
    let y = b.y;
    let alive = true;
    for (let s = 0; s < steps && alive; s++) {
      x += (b.vx * dt) / steps;
      y += (b.vy * dt) / steps;
      if (x < -1 || x > COLS + 1 || y < FIELD_TOP - 2 || y > FIELD_BOTTOM + 2) {
        alive = false;
        break;
      }
      if (t.obstacles.some((o) => o.solid && inside(x, y, o))) {
        particles = [...particles, ...burst(x, y, 3, "spark", 10, 0.25)];
        alive = false;
        break;
      }
      const idx = animals.findIndex((a) => !a.dead && inside(x, y, { x: a.x, y: a.y, w: ANIMALS[a.kind].w, h: ANIMALS[a.kind].h }));
      if (idx >= 0) {
        const a = animals[idx];
        const spec = ANIMALS[a.kind];
        const cx = a.x + spec.w / 2;
        const cy = a.y + spec.h / 2;
        if (a.hp > 1) {
          // Winged it: it bolts, and you'll need a second round. A bear does
          // worse than bolt — it rears up and comes for you.
          const bear = a.kind === "bear";
          animals = animals.map((o, i) => (i === idx ? { ...o, hp: o.hp - 1, spooked: true, moving: true, charging: bear, rear: bear ? REAR_UP : 0, retarget: 0 } : o));
          popups = [...popups, { x: cx * U, y: a.y * U - 2, text: "HIT!", life: 0.6, max: 0.6 }];
          particles = [...particles, ...burst(cx, cy, 4, "dust", 8, 0.4)];
          sfx = pushSfx(sfx, "hit", ...(bear ? (["growl"] as const) : []));
        } else {
          animals = animals.map((o, i) => (i === idx ? { ...o, hp: 0, dead: true, moving: false, charging: false } : o));
          shotLbs += spec.lbs;
          kills = [...kills, { kind: a.kind, lbs: spec.lbs }];
          hitStop = 0.06 * motionScale();
          if (spec.lbs >= 100) newShake = 0.25;
          sfx = pushSfx(sfx, spec.lbs >= 100 ? "bigKill" : "kill");
          popups = [...popups, { x: cx * U, y: a.y * U - 2, text: `+${spec.lbs} lb`, life: 0.9, max: 0.9, big: spec.lbs >= 50 }];
          particles = [...particles, ...burst(cx, cy, spec.lbs >= 100 ? 10 : 6, "dust", 10, 0.5)];
          if (shotLbs >= t.quota && !endIn) {
            endIn = 0.8;
            sfx = pushSfx(sfx, "full");
            popups = [...popups, { x: (COLS * U) / 2, y: (ROWS * U) / 2, text: "FULL BAG!", life: 1, max: 1, big: true }];
          }
        }
        alive = false;
      }
    }
    if (alive) bullets.push({ ...b, x, y });
  }

  // A charging bear that reaches you knocks you flat.
  let mauledIn = 0;
  const hr = hunterRect(hunter);
  const mauler = animals.find((a) => !a.dead && a.charging && (a.rear ?? 0) <= 0 && overlaps(hr, { x: a.x, y: a.y, w: ANIMALS[a.kind].w, h: ANIMALS[a.kind].h }));
  if (mauler && !endIn) {
    mauledIn = KNOCKDOWN;
    hunter = { ...hunter, down: KNOCKDOWN, walking: false };
    newShake = 0.3;
    sfx = pushSfx(sfx, "bad");
    particles = [...particles, ...burst(hunter.x, hunter.y, 8, "dust", 10, 0.5)];
    popups = [...popups, { x: hunter.x * U, y: (hunter.y - 4) * U, text: "MAULED!", life: 1, max: 1, big: true }];
  }

  // The last five seconds tick down out loud.
  let tickAt = t.tickAt;
  const remaining = t.seconds - elapsed;
  const secLeft = Math.ceil(remaining);
  if (remaining <= 5 && remaining > 0 && secLeft !== tickAt) {
    tickAt = secLeft;
    sfx = pushSfx(sfx, "tick");
  }

  let done = false;
  let doneReason: DoneReason = "";
  if (endIn > 0) {
    endIn -= dt;
    if (endIn <= 0) {
      done = true;
      doneReason = "full";
    }
  }
  if (!done && elapsed >= t.seconds) {
    done = true;
    doneReason = "time";
  }
  if (!done && t.bulletsLeft === 0 && bullets.length === 0 && !endIn && !mauledIn) {
    done = true;
    doneReason = "ammo";
  }

  return {
    ...t,
    clock,
    hunter,
    heldFor,
    swipeWalk,
    animals,
    bullets,
    particles,
    popups,
    shake: newShake,
    fireCooldown,
    elapsed: Math.min(elapsed, t.seconds),
    nextSpawn,
    shotLbs,
    kills,
    hitStop,
    endIn,
    done,
    doneReason,
    sfx,
    tickAt,
    mauledIn,
  };
}

export function tripResult(t: HuntTrip) {
  const carried = Math.min(t.shotLbs, t.carryCap);
  return { shot: t.shotLbs, carried, wasted: t.shotLbs - carried, bulletsUsed: t.bulletsUsed, kills: t.kills, mauled: t.doneReason === "mauled" };
}

/** Freeze hold-to-move input when the host pauses (window blur): releases are
    lost while unfocused, so the hunter must not keep walking on resume. */
export function tripPause(t: HuntTrip): HuntTrip {
  return { ...t, held: [], heldFor: {}, swipeWalk: null, hunter: { ...t.hunter, walking: false } };
}

// ---- rendering: the field -------------------------------------------------------

function Line({ x, y, c, size = 8, anchor, children }: { x: number; y: number; c: string; size?: number; anchor?: "start" | "middle" | "end"; children: string }) {
  return (
    <text x={x} y={y} fill={c} fontFamily="var(--font-readout)" fontSize={size} textAnchor={anchor} xmlSpace="preserve">
      {children}
    </text>
  );
}

const W = COLS * U;
const H = ROWS * U;

function groundColor(terrain: Terrain, c: SceneColors) {
  return terrain === "desert" ? c.mountain : c.grass;
}

function ObstacleSprite({ o, c }: { o: Obstacle; c: SceneColors }) {
  const { x, y, w, h } = o;
  switch (o.kind) {
    case "tree":
      return (
        <g>
          <Px x={x} y={y} w={w} h={3} fill={c.line} />
          <Px x={x + 1} y={y - 1} w={w - 2} h={1} fill={c.line} />
          <Px x={x + 1} y={y + 1} w={1} h={1} fill={c.snow} o={0.25} />
          <Px x={x + 2} y={y + 3} w={1} h={2} fill={c.mountain} />
        </g>
      );
    case "pine":
      return (
        <g>
          {poly([[x + 1.5, y - 1], [x + w, y + 3], [x, y + 3]], c.line)}
          <Px x={x + 1} y={y + 3} w={1} h={2} fill={c.mountain} />
        </g>
      );
    case "rock":
      return (
        <g>
          <Px x={x} y={y} w={w} h={h} fill={c.mountainBack} />
          <Px x={x + 1} y={y - 1} w={w - 2} h={1} fill={c.mountainBack} />
          <Px x={x} y={y} w={1} h={1} fill={c.snow} o={0.3} />
        </g>
      );
    case "cactus":
      return (
        <g>
          <Px x={x} y={y} w={1} h={h} fill={c.line} />
          <Px x={x - 1} y={y + 1} w={1} h={1} fill={c.line} />
          <Px x={x + 1} y={y + 2} w={1} h={1} fill={c.line} />
          <Px x={x - 1} y={y} w={1} h={1} fill={c.line} />
          <Px x={x + 1} y={y + 1} w={1} h={1} fill={c.line} />
        </g>
      );
    case "shrub":
      return (
        <g>
          <Px x={x} y={y} w={w} h={h} fill={c.line} o={0.6} />
          <Px x={x + 1} y={y - 1} w={1} h={1} fill={c.line} o={0.6} />
        </g>
      );
    default:
      return (
        <g>
          <Px x={x} y={y} w={w} h={1} fill={c.sky} o={0.35} />
          <Px x={x + 1} y={y - 1} w={1} h={1} fill={c.sky} o={0.35} />
        </g>
      );
  }
}

/** Animal pixel art in local cells (origin: bounding-box top-left, facing
    right). `step` alternates the legs. */
function animalPixels(kind: AnimalKind, step: number, c: SceneColors, tell = false, rear = false): React.JSX.Element {
  const legs = (xs: number[], y: number, fill: string) => xs.map((lx, i) => <Px key={`l${i}`} x={lx + ((i + step) % 2)} y={y} w={1} h={1} fill={fill} />);
  switch (kind) {
    case "squirrel":
      return (
        <g>
          <Px x={0} y={1} w={2} h={1} fill={c.mountain} />
          <Px x={0} y={0} w={1} h={1} fill={c.mountain} o={0.8} />
          <Px x={1} y={0} w={1} h={1} fill={c.snow} o={0.5} />
        </g>
      );
    case "rabbit":
      return (
        <g>
          <Px x={0} y={1} w={2} h={1} fill={c.snow} />
          <Px x={1} y={0} w={1} h={1} fill={c.snow} />
          <Px x={0} y={0} w={1} h={1} fill={c.snow} o={0.5} />
        </g>
      );
    case "deer":
      return (
        <g>
          <Px x={0} y={1} w={4} h={1} fill={c.accent} />
          <Px x={3} y={0} w={1} h={1} fill={c.accent} />
          <Px x={4} y={-1} w={1} h={1} fill={c.line} o={0.8} />
          {/* the tell: ears up before it bounds */}
          {tell && <Px x={3} y={-1} w={1} h={1} fill={c.accent} />}
          {tell && <Px x={2} y={-1} w={1} h={1} fill={c.accent} o={0.7} />}
          {legs([0, 2], 2, c.accent)}
        </g>
      );
    case "bear":
      if (rear) {
        // Reared up on its hind legs, paws raised: it's coming.
        return (
          <g>
            <Px x={1} y={-1} w={3} h={1} fill={c.line} />
            <Px x={1} y={0} w={3} h={2} fill={c.line} />
            <Px x={0} y={-1} w={1} h={1} fill={c.line} />
            <Px x={4} y={-1} w={1} h={1} fill={c.line} />
            <Px x={3} y={-1} w={1} h={1} fill={c.sky} o={0.4} />
            <Px x={1} y={2} w={1} h={1} fill={c.line} />
            <Px x={3} y={2} w={1} h={1} fill={c.line} />
          </g>
        );
      }
      return (
        <g>
          <Px x={0} y={1} w={5} h={1} fill={c.line} />
          <Px x={1} y={0} w={4} h={1} fill={c.line} />
          <Px x={4} y={0} w={1} h={1} fill={c.sky} o={0.4} />
          {legs([0, 3], 2, c.line)}
        </g>
      );
    default:
      return (
        <g>
          <Px x={0} y={1} w={6} h={1} fill={c.mountain} />
          <Px x={1} y={0} w={3} h={1} fill={c.mountain} />
          <Px x={5} y={0} w={1} h={1} fill={c.sky} o={0.5} />
          <Px x={5} y={-1} w={1} h={1} fill={c.snow} o={0.8} />
          {legs([0, 2, 4], 2, c.mountain)}
        </g>
      );
  }
}

function AnimalSprite({ a, c }: { a: Animal; c: SceneColors }) {
  const spec = ANIMALS[a.kind];
  const px = Math.round(a.x);
  const py = Math.round(a.y);
  const cx = (px + spec.w / 2) * U;
  const cy = (py + spec.h / 2) * U;
  const faceLeft = a.vx < 0;
  const step = a.moving && !a.dead ? Math.floor(a.age * (a.charging ? 12 : 8)) % 2 : 0;
  // The legs-in-the-air flip, exactly as the Apple II did it.
  const flip = a.dead ? `translate(${cx} ${cy}) scale(1 -1) translate(${-cx} ${-cy})` : "";
  const mirror = faceLeft ? `translate(${cx} ${cy}) scale(-1 1) translate(${-cx} ${-cy})` : "";
  return (
    <g transform={`${flip} ${mirror} translate(${px * U} ${py * U})`.trim()} opacity={a.dead ? 0.85 : 1}>
      {animalPixels(a.kind, step, c, !a.dead && (a.tell ?? 0) > 0, !a.dead && (a.rear ?? 0) > 0)}
    </g>
  );
}

function HunterSprite({ h, c }: { h: Hunter; c: SceneColors }) {
  const px = Math.round(h.x - (h.recoil > 0 ? h.fx * 0.6 : 0));
  const py = Math.round(h.y - (h.recoil > 0 ? h.fy * 0.6 : 0));
  if (h.down > 0) {
    // Flat on the ground, hat off, rifle dropped.
    return (
      <g>
        <Px x={px - 2} y={py} w={5} h={1} fill={c.text} />
        <Px x={px - 3} y={py} w={1} h={1} fill={c.snow} />
        <Px x={px - 4} y={py - 1} w={1} h={1} fill={c.mountain} />
        <Px x={px} y={py + 1} w={3} h={1} fill={c.sky} />
      </g>
    );
  }
  const rifle = [];
  for (let i = 1; i <= 3; i++) rifle.push(<Px key={i} x={px + h.fx * i} y={py + h.fy * i} w={1} h={1} fill={c.sky} />);
  return (
    <g>
      <Px x={px - 1} y={py - 2} w={3} h={1} fill={c.mountain} />
      <Px x={px} y={py - 1} w={1} h={1} fill={c.snow} />
      <Px x={px - 1} y={py} w={3} h={1} fill={c.text} />
      <Px x={px - 1} y={py + 1} w={1} h={1} fill={c.text} />
      <Px x={px + 1} y={py + 1} w={1} h={1} fill={c.text} />
      {rifle}
      {h.flash > 0 && <Px x={px + h.fx * 4 - 1} y={py + h.fy * 4 - 1} w={3} h={3} fill={c.accent} o={0.9 * motionScale()} />}
    </g>
  );
}

// Rain streak columns for the western forest, laid out once.
const RAIN: [number, number][] = Array.from({ length: 12 }, (_, i) => [((i * 37) % (COLS - 2)) + 1, (i * 11) % ROWS]);

/** Time of day and weather, keyed by zone: morning mist in the east, high noon
    on the plains, dusk in the mountains, heat in the desert, rain out west.
    Cheap overlays drawn under the HUD — the sky is the only thing that
    changes, so five trips stop looking like five green rectangles. */
function Weather({ t, c }: { t: HuntTrip; c: SceneColors }) {
  const k = t.clock;
  switch (t.terrain) {
    case "eastForest": {
      const drift = Math.floor((k * 1.5) % COLS);
      return (
        <g opacity={0.22}>
          <Px x={drift - COLS} y={8} w={COLS} h={3} fill={c.snow} />
          <Px x={drift} y={8} w={COLS} h={3} fill={c.snow} />
          <Px x={COLS - drift * 0.6 - COLS} y={20} w={COLS} h={2} fill={c.snow} />
          <Px x={COLS - drift * 0.6} y={20} w={COLS} h={2} fill={c.snow} />
        </g>
      );
    }
    case "plains":
      return <Px x={0} y={0} w={COLS} h={ROWS} fill={c.snow} o={0.05} />;
    case "mountains":
      return (
        <g>
          <Px x={0} y={0} w={COLS} h={ROWS} fill={c.accent} o={0.12} />
          <Px x={0} y={ROWS - 8} w={COLS} h={8} fill="#000" o={0.18} />
        </g>
      );
    case "desert":
      return (
        <g>
          <Px x={0} y={0} w={COLS} h={ROWS} fill={c.snow} o={0.06 + 0.03 * Math.sin(k * 5)} />
          <Px x={COLS - 6} y={4} w={3} h={3} fill={c.sun} o={0.8} />
          <Px x={COLS - 7} y={5} w={1} h={1} fill={c.sun} o={0.5} />
          <Px x={COLS - 3} y={5} w={1} h={1} fill={c.sun} o={0.5} />
          <Px x={COLS - 5} y={3} w={1} h={1} fill={c.sun} o={0.5} />
        </g>
      );
    default: {
      const fall = Math.floor(k * 22);
      return (
        <g>
          <Px x={0} y={0} w={COLS} h={ROWS} fill="#4a5a7a" o={0.08} />
          {RAIN.map(([x, y0], i) => (
            <Px key={i} x={x} y={(y0 + fall + i) % ROWS} w={1} h={2} fill={c.snow} o={0.35} />
          ))}
        </g>
      );
    }
  }
}

/** The field contents for one trip, to drop inside a <GameFrame>. */
export function HuntScene({ t, p, hud = true }: { t: HuntTrip; p: ThemePalette; hud?: boolean }) {
  const c = sceneColors(p);
  const remaining = Math.max(0, t.seconds - t.elapsed);
  const frac = t.seconds > 0 ? remaining / t.seconds : 0;
  const urgent = remaining <= 5 && t.warmup <= 0;
  const blink = urgent && Math.floor(t.clock * 4) % 2 === 0;
  const carried = Math.min(t.shotLbs, t.carryCap);
  const quota = t.quota ?? t.carryCap;
  return (
    <g transform={shakeTransform(t.shake)}>
      <Px x={0} y={0} w={COLS} h={ROWS} fill={groundColor(t.terrain, c)} />
      {t.terrain === "mountains" && <Px x={0} y={0} w={COLS} h={ROWS} fill={c.mountainBack} o={0.15} />}
      <Weather t={t} c={c} />
      {t.obstacles.filter((o) => !o.solid).map((o, i) => <ObstacleSprite key={`d${i}`} o={o} c={c} />)}
      {t.animals.filter((a) => a.dead).map((a) => <AnimalSprite key={a.id} a={a} c={c} />)}
      {t.animals.filter((a) => !a.dead).map((a) => <AnimalSprite key={a.id} a={a} c={c} />)}
      {t.obstacles.filter((o) => o.solid).map((o, i) => <ObstacleSprite key={`s${i}`} o={o} c={c} />)}
      <HunterSprite h={t.hunter} c={c} />
      {t.bullets.map((b) => {
        const len = Math.hypot(b.vx, b.vy) || 1;
        return (
          <g key={b.id}>
            <Px x={Math.round(b.x - (b.vx / len) * 2)} y={Math.round(b.y - (b.vy / len) * 2)} w={1} h={1} fill={c.accent} o={0.4} />
            <Px x={Math.round(b.x)} y={Math.round(b.y)} w={1} h={1} fill={c.accent} />
          </g>
        );
      })}
      <Particles ps={t.particles} fill={c.snow} spark={c.accent} />
      <Popups ps={t.popups} fill={c.text} shadow="#000" />
      {hud && (
        <g>
          <Px x={0} y={0} w={COLS} h={3} fill="#000" o={0.55} />
          <rect x={4} y={2} width={W - 8} height={2} fill={c.line} opacity={0.4} />
          <rect x={4} y={2} width={(W - 8) * frac} height={2} fill={urgent ? c.danger : c.grass} opacity={blink ? 0.35 : 1} />
          <Line x={4} y={11} c={t.bulletsLeft <= 3 ? c.danger : c.text} size={7}>{`BULLETS ${t.bulletsLeft}`}</Line>
          <Line x={W / 2} y={11} c={urgent ? c.danger : c.text} size={7} anchor="middle">{`${Math.ceil(remaining)}s`}</Line>
          <Line x={W - 4} y={11} c={carried >= quota ? c.accent : c.text} size={7} anchor="end">{`MEAT ${carried}/${quota} lb`}</Line>
        </g>
      )}
    </g>
  );
}

// ---- the arcade cabinet: a five-trip season -----------------------------------

type Phase = "trip" | "card" | "over";

export interface HuntState {
  phase: Phase;
  tripIndex: number;
  trip: HuntTrip;
  score: number;
  best: number;
  newBest: boolean;
  cardTitle: string;
  cardLines: string[];
  /** The season stops early when the ammo box is empty. */
  seasonOver: boolean;
  remark: string;
  clock: number;
  /** Mirrors the live trip's cue queue so the wrapper can hear it. */
  sfx: SfxEvent[];
}

const SEASON_BULLETS = 40;
const TRIP_SECONDS = 30;
const CARRY_CAP = 100;
const WARMUP = 0.6;

const REMARKS = [
  "The wagon eats tonight.",
  "The buffalo send their regards.",
  "Squirrel stew again.",
  "Legs up. Every time.",
  "Your aim is the talk of the fort.",
  "Nobody tell the oxen.",
];

function startTrip(index: number, bullets: number, held: string[] = [], heldFor: Record<string, number> = {}): HuntTrip {
  const terrain = TERRAIN_ORDER[index];
  const trip = newTrip({ terrain, bullets, seconds: TRIP_SECONDS, carryCap: CARRY_CAP, warmup: WARMUP, quota: quotaFor(terrain) });
  const [dx, dy] = heldVector(held);
  return { ...trip, held, heldFor, hunter: dx || dy ? { ...trip.hunter, fx: dx, fy: dy } : trip.hunter };
}

function freshSeason(best: number): HuntState {
  return {
    phase: "trip",
    tripIndex: 0,
    trip: startTrip(0, SEASON_BULLETS),
    score: 0,
    best,
    newBest: false,
    cardTitle: "",
    cardLines: [],
    seasonOver: false,
    remark: "",
    clock: 0,
    sfx: [],
  };
}

function finishTrip(s: HuntState): HuntState {
  const t = s.trip;
  const r = tripResult(t);
  const bonus = t.doneReason === "full" ? Math.round(t.seconds - t.elapsed) * 2 : 0;
  const score = s.score + r.carried + bonus;
  const lines = [`Carried ${r.carried} lb of ${r.shot} lb shot`];
  if (t.quota < t.carryCap) lines.push(`${TERRAIN_NAMES[t.terrain].split(" ").pop()} quota: ${t.quota} lb`);
  if (r.wasted > 0) lines.push(`${r.wasted} lb left to waste`);
  if (bonus > 0) lines.push(`Full bag bonus +${bonus}`);
  if (t.doneReason === "mauled") lines.push("A bear got you — trip cut short");
  if (t.doneReason === "ammo") lines.push("Out of bullets — season over");
  lines.push(`Score ${score}`);
  const trip = { ...t, sfx: pushSfx(t.sfx, r.carried > 0 ? "good" : "bad") };
  return {
    ...s,
    phase: "card",
    trip,
    sfx: trip.sfx,
    score,
    cardTitle: `TRIP ${s.tripIndex + 1} · ${TERRAIN_NAMES[t.terrain].toUpperCase()}`,
    cardLines: lines,
    seasonOver: t.doneReason === "ammo" || s.tripIndex >= TERRAIN_ORDER.length - 1,
  };
}

function endSeason(s: HuntState): HuntState {
  const score = s.score + s.trip.bulletsLeft * 3;
  const newBest = score > s.best;
  const best = newBest ? saveBest("hunt", score) : s.best;
  const sfx = newBest ? pushSfx(s.sfx, "best") : s.sfx;
  return { ...s, phase: "over", score, best, newBest, remark: pick(REMARKS), sfx };
}

function huntKey(s: HuntState, key: string): HuntState {
  switch (s.phase) {
    case "trip": {
      const trip = tripKey(s.trip, key);
      return { ...s, trip, sfx: trip.sfx };
    }
    case "card": {
      if (s.seasonOver) return endSeason(s);
      const index = s.tripIndex + 1;
      const trip = startTrip(index, s.trip.bulletsLeft, s.trip.held, s.trip.heldFor);
      // The dismissing key doubles as your first step onto the next field.
      return { ...s, phase: "trip", tripIndex: index, trip: ARROWS.includes(key) ? tripKey(trip, key) : trip, sfx: [] };
    }
    default:
      return freshSeason(s.best);
  }
}

function huntKeyUp(s: HuntState, key: string): HuntState {
  return { ...s, trip: tripKeyUp(s.trip, key) };
}

/** Taps fire and swipes walk on the field; on a card a tap turns the page. */
function huntPointer(s: HuntState, p: PointerInput): HuntState {
  if (s.phase !== "trip") return huntKey(s, "Enter");
  const trip = tripPointer(s.trip, p);
  return { ...s, trip, sfx: trip.sfx };
}

function huntUpdate(s: HuntState, dt: number): HuntState {
  const clock = s.clock + dt;
  if (s.phase !== "trip") return { ...s, clock };
  const trip = tripUpdate(s.trip, dt);
  if (trip.done) return finishTrip({ ...s, clock, trip });
  return { ...s, clock, trip, sfx: trip.sfx };
}

function Card({ title, lines, c, foot }: { title: string; lines: string[]; c: SceneColors; foot: string }) {
  const h = 40 + lines.length * 11;
  const y = (H - h) / 2;
  return (
    <g>
      <rect x={14} y={y} width={W - 28} height={h} fill="#000" opacity={0.78} />
      <rect x={14} y={y} width={W - 28} height={h} fill="none" stroke={c.accent} strokeWidth={1} opacity={0.9} />
      <PxText x={W / 2} y={y + 17} size={11} fill={c.accent} shadow="#000" anchor="middle">{title}</PxText>
      {lines.map((line, i) => (
        <Line key={i} x={W / 2} y={y + 32 + i * 11} c={c.text} size={8} anchor="middle">{line}</Line>
      ))}
      <Line x={W / 2} y={H - 10} c={c.text} size={7} anchor="middle">{foot}</Line>
    </g>
  );
}

function HuntRender(s: HuntState, p: ThemePalette) {
  const c = sceneColors(p);
  const t = s.trip;
  const label =
    s.phase === "over"
      ? "Hunting Season: the season is over"
      : s.phase === "card"
        ? `Hunting Season: ${s.cardTitle}`
        : `Hunting Season: hunting in the ${TERRAIN_NAMES[t.terrain]}`;
  const ready = s.phase === "trip" && t.warmup > 0;
  const justStarted = s.phase === "trip" && t.warmup <= 0 && t.elapsed < 0.6;
  return (
    <GameFrame label={label}>
      <HuntScene t={t} p={p} hud={s.phase === "trip"} />
      {/* The footer is the game's own (no wrapper help line): controls for the
          first few seconds of a trip, then the season tally. */}
      {s.phase === "trip" && (
        <g>
          <Px x={0} y={ROWS - 3} w={COLS} h={3} fill="#000" o={0.45} />
          <Line x={W / 2} y={H - 4} c={c.text} size={6} anchor="middle">
            {t.elapsed < 4 ? "← ↑ ↓ → walk · space fires · esc makes camp" : `TRIP ${s.tripIndex + 1}/${TERRAIN_ORDER.length} · ${TERRAIN_NAMES[t.terrain].toUpperCase()} · SCORE ${s.score}`}
          </Line>
        </g>
      )}
      {ready && (
        <PxText x={W / 2} y={H / 2 - 10} size={14} fill={c.accent} shadow="#000" anchor="middle">READY…</PxText>
      )}
      {justStarted && (
        <PxText x={W / 2} y={H / 2 - 10} size={16} fill={c.accent} shadow="#000" anchor="middle">HUNT!</PxText>
      )}
      {s.phase === "card" && <Card title={s.cardTitle} lines={s.cardLines} c={c} foot={s.seasonOver ? "press any key to tally the season" : "press any key for the next trip"} />}
      {s.phase === "over" && (
        <g>
          <rect x={0} y={0} width={W} height={H} fill="#000" opacity={0.55} />
          <PxText x={W / 2} y={30} size={14} fill={c.accent} shadow="#000" anchor="middle">SEASON OVER</PxText>
          <PxText x={W / 2} y={58} size={16} fill={c.text} shadow="#000" anchor="middle">{`SCORE ${s.score}`}</PxText>
          {s.newBest && Math.floor(s.clock * 3) % 2 === 0 ? (
            <PxText x={W / 2} y={76} size={10} fill={c.grass} shadow="#000" anchor="middle">NEW BEST!</PxText>
          ) : (
            <Line x={W / 2} y={76} c={c.text} size={8} anchor="middle">{`BEST ${s.best}`}</Line>
          )}
          <Line x={W / 2} y={96} c={c.accent} size={8} anchor="middle">{s.remark}</Line>
          <Line x={W / 2} y={H - 10} c={c.text} size={7} anchor="middle">press any key for a new season</Line>
        </g>
      )}
    </GameFrame>
  );
}

/** Attract screen: a hunter on the plains, buffalo on the horizon. */
function HuntAttract(p: ThemePalette) {
  const c = sceneColors(p);
  const still = { hp: 1, dead: false, timer: 0, moving: false, age: 0, spooked: false, tell: 0, charging: false, rear: 0, retarget: 0 };
  const buffalo: Animal = { ...still, id: 0, kind: "buffalo", x: 44, y: 21, vx: -1, vy: 0, hp: 2 };
  const rabbit: Animal = { ...still, id: 0, kind: "rabbit", x: 14, y: 26, vx: 1, vy: 0 };
  const hunter: Hunter = { x: 28, y: 23, fx: 1, fy: 0, walking: false, recoil: 0, flash: 0, down: 0 };
  return (
    <GameFrame label="Hunting Season title screen: a hunter faces a buffalo on the plains">
      <Px x={0} y={0} w={COLS} h={ROWS} fill={c.grass} />
      <Px x={0} y={0} w={COLS} h={12} fill={c.sky} />
      <Px x={60} y={3} w={4} h={4} fill={c.sun} />
      {poly([[0, 12], [14, 6], [26, 11], [40, 5], [54, 10], [66, 7], [72, 11], [72, 12]], c.mountain)}
      {[[6, 16], [20, 30], [50, 15], [62, 28], [36, 29]].map(([x, y], i) => (
        <ObstacleSprite key={i} o={{ kind: "tuft", x, y, w: 2, h: 1, solid: false }} c={c} />
      ))}
      <AnimalSprite a={buffalo} c={c} />
      <AnimalSprite a={rabbit} c={c} />
      <HunterSprite h={hunter} c={c} />
      <PxText x={W / 2} y={7 * U} size={15} fill={c.accent} shadow="#000" anchor="middle">HUNTING SEASON</PxText>
      <PxText x={W / 2} y={11 * U} size={7} fill={c.text} shadow="#000" anchor="middle">Bring back 100 lb · arrows or swipe walk · space or tap fires</PxText>
    </GameFrame>
  );
}

export const HuntGame: HeroGameDefinition<HuntState> = {
  title: "Hunting Season",
  tab: "Hunt",
  seedSalt: 0x4a4e,
  initialState: () => freshSeason(loadBest("hunt")),
  onStart: (s) => freshSeason(Math.max(s.best, loadBest("hunt"))),
  handleKey: huntKey,
  handleKeyUp: huntKeyUp,
  handlePointer: huntPointer,
  onPause: (s) => ({ ...s, trip: tripPause(s.trip) }),
  update: huntUpdate,
  render: HuntRender,
  renderAttract: HuntAttract,
  keys: (key) => key.length === 1 || ["Enter", ...ARROWS].includes(key),
  // No wrapper help line: the trip footer carries the controls itself.
};
