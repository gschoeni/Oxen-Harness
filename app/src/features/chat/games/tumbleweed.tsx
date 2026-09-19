// Tumbleweed Dodge — an 8-bit endless runner. Steer the ox down a converging
// Oregon-Trail vista, dodging tumbleweeds that roll down toward you. The attract
// screen doubles as a title card built from the old hero's trail tableau.
//
// The design follows the "simple and addicting" arcade playbook: one big score
// next to your BEST, a combo multiplier you keep alive by grazing danger and
// chaining coins, difficulty that comes in waves with short breathers, a rare
// horseshoe shield as a variable reward, hop as a second verb that later logs
// make necessary, and death that is juicy (hit-stop, shake, debris, a tumbling
// ox) but never slow — any arrow key restarts instantly.

import { Pine, TitlePlaque, Vista } from "./arcadeArt";
import type { ThemePalette } from "../../../lib/types";
import {
  burst,
  clamp,
  COLS,
  GameFrame,
  loadBest,
  loadPref,
  motionScale,
  Particles,
  pickRand,
  pointerAsKey,
  Popups,
  pushSfx,
  Px,
  poly,
  PxText,
  rand,
  ROWS,
  saveBest,
  savePref,
  sceneColors,
  SceneColors,
  shakeTransform,
  stepParticles,
  stepPopups,
  U,
  type HeroGameDefinition,
  type Particle,
  type PointerInput,
  type Popup,
  type SfxEvent,
} from "./gameKit";

type Phase = "ready" | "run" | "dead";
type ObstacleKind = "weed" | "big" | "bouncer" | "log";
type PickupKind = "coin" | "shield";
type Rider = "ox" | "mule" | "wagon";

interface Obstacle {
  id: number;
  kind: ObstacleKind;
  lane: number; // logical lane (target lane for bouncers)
  laneVis: number; // rendered lane, tweens when a bouncer switches
  row: number;
  speed: number;
  spin: number;
  /** Bouncers switch lanes once at BOUNCE_ROW; they wobble beforehand. */
  bounced: boolean;
  bounceDir: number;
  /** Set once the ox has been rewarded for grazing/hopping this obstacle. */
  scored: boolean;
  /** Bouncers puff dust once as a tell before they switch lanes. */
  puffed: boolean;
}

/** What a crash leaves behind: a broken weed and the ox's hat, scrolling off
    with the trail on the next run. */
interface Wreck {
  lane: number;
  row: number;
}

interface Pickup {
  id: number;
  kind: PickupKind;
  lane: number;
  row: number;
}

interface RunnerState {
  phase: Phase;
  time: number; // seconds in this run
  deadFor: number; // seconds since the crash (drives the card + tumble)

  // the ox
  oxLane: number; // target lane
  laneVis: number; // rendered lane, tweens toward oxLane
  queuedLane: number | null; // input buffered during a tween
  squash: number; // seconds of landing squash left
  hop: number; // seconds left airborne (0 = grounded)
  hopCooldown: number;
  hopBuffer: number;
  invincible: number;
  courage: number;
  stampede: number;
  shield: boolean;

  // the trail
  obstacles: Obstacle[];
  pickups: Pickup[];
  nextSpawn: number;
  nextCoin: number;
  nextShieldIn: number;
  wave: number;
  waveT: number;
  dist: number; // yards rolled — drives scroll, speed, and milestones
  milestone: number;
  banner: { text: string; life: number } | null;
  slowMo: number;

  // scoring
  score: number;
  best: number;
  newBest: boolean;
  mult: number;
  maxMult: number;
  multTimer: number; // seconds until the multiplier decays a step
  coinChain: number;

  // juice
  particles: Particle[];
  popups: Popup[];
  hitStop: number;
  shake: number;
  deathLine: string;
  sfx: SfxEvent[];
  wrecks: Wreck[];

  // the rider (cosmetic; unlocks are meta-progression)
  rider: Rider;
  unlocked: Rider[];
}

const HZ = 11; // horizon row: sky + mountains above, prairie below
const OX_ROW = 27;

// The trail converges toward the horizon; a lane's x depends on its row.
const LANES_BOTTOM = [21, 36, 51];
const LANES_HORIZON = [31.5, 36, 40.5];
const RUTS_BOTTOM = [28.5, 43.5];
const RUTS_HORIZON = [33.8, 38.2];

const LANE_TWEEN = 0.09; // seconds per lane change
const HOP_TIME = 0.42;
const HOP_COOLDOWN = 0.75;
const HOP_BUFFER = 0.14;
const STAMPEDE_SECONDS = 5;
const HOP_HEIGHT = 5; // cells at the apex
const BOUNCE_ROW = HZ + 9;
const MULT_MAX = 8;
const MULT_DECAY = 3; // seconds without a bonus before the multiplier drops
const WAVE_LEN = 15; // seconds: 12 rising, then a 3-second breather
const BREATHER = 3;
const READY_TIME = 0.8;
const BASE_SPEED = 11; // rows/s at wave 0
const MAX_SPEED = 24;

const LANDMARKS = ["Fort Kearney", "Chimney Rock", "Fort Laramie", "Independence Rock", "South Pass", "Fort Hall", "Snake River", "The Dalles", "Oregon City"];
const LANDMARK_EVERY = 300;
const RIDERS: { id: Rider; label: string; yards: number }[] = [
  { id: "ox", label: "OX", yards: 0 },
  { id: "mule", label: "MULE", yards: 1000 },
  { id: "wagon", label: "WAGON", yards: 2000 },
];

// Riders and their unlocks persist between sessions.
function loadUnlocked(): Rider[] {
  const raw = loadPref("dodge-unlocks");
  const list = (raw ? raw.split(",") : []).filter((r): r is Rider => RIDERS.some((d) => d.id === r));
  return list.includes("ox") ? list : ["ox", ...list];
}

function loadRider(unlocked = loadUnlocked()): Rider {
  const raw = loadPref("dodge-rider") as Rider | null;
  return raw && unlocked.includes(raw) ? raw : "ox";
}

const DEATH_LINES = ["Ox 0 · Weed 1", "Should've hopped.", "Tumbled, not stirred.", "The prairie wins this round.", "Dysentery would've been quicker.", "That weed had your name on it."];

let nextId = 1;

function laneX(lane: number, row: number) {
  const t = clamp((row - HZ) / (ROWS - HZ), 0, 1);
  const l = clamp(lane, 0, 2);
  const lo = Math.floor(l);
  const hi = Math.min(2, lo + 1);
  const f = l - lo;
  const bottom = LANES_BOTTOM[lo] + (LANES_BOTTOM[hi] - LANES_BOTTOM[lo]) * f;
  const horizon = LANES_HORIZON[lo] + (LANES_HORIZON[hi] - LANES_HORIZON[lo]) * f;
  return horizon + (bottom - horizon) * t;
}

function speedFor(wave: number) {
  return Math.min(MAX_SPEED, BASE_SPEED + wave * 1.6);
}

function resetRunner(best = 0, wrecks: Wreck[] = []): RunnerState {
  const unlocked = loadUnlocked();
  return {
    phase: "ready",
    time: 0,
    deadFor: 0,
    oxLane: 1,
    laneVis: 1,
    queuedLane: null,
    squash: 0,
    hop: 0,
    hopCooldown: 0,
    hopBuffer: 0,
    invincible: 0,
    courage: 0,
    stampede: 0,
    shield: false,
    obstacles: [],
    pickups: [],
    nextSpawn: READY_TIME + 0.6,
    nextCoin: READY_TIME + 1.4,
    nextShieldIn: 12 + rand() * 6,
    wave: 0,
    waveT: 0,
    dist: 0,
    milestone: 0,
    banner: null,
    slowMo: 0,
    score: 0,
    best,
    newBest: false,
    mult: 1,
    maxMult: 1,
    multTimer: MULT_DECAY,
    coinChain: 0,
    particles: [],
    popups: [],
    hitStop: 0,
    shake: 0,
    deathLine: "",
    sfx: [],
    wrecks,
    rider: loadRider(unlocked),
    unlocked,
  };
}

// ---- input -----------------------------------------------------------------

const RESTART_KEYS = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " ", "Enter"];

function runnerKey(state: RunnerState, key: string): RunnerState {
  if (state.phase === "dead") {
    // A short grace period so the key you were mashing at the moment of the
    // crash doesn't skip the card before you've seen your score.
    if (state.deadFor > 0.35 && RESTART_KEYS.includes(key)) return restart(state);
    return state;
  }

  if (key === "ArrowLeft" || key === "ArrowRight") {
    const dir = key === "ArrowLeft" ? -1 : 1;
    const moving = Math.abs(state.laneVis - state.oxLane) > 0.02;
    // Buffer a press that lands mid-tween: it plays as soon as the ox arrives.
    if (moving) return { ...state, queuedLane: clamp(state.oxLane + dir, 0, LANES_BOTTOM.length - 1) };
    return { ...state, oxLane: clamp(state.oxLane + dir, 0, LANES_BOTTOM.length - 1), queuedLane: null };
  }
  if (key === "ArrowUp" || key === " ") {
    if (state.hop > 0 || state.hopCooldown > 0) return { ...state, hopBuffer: HOP_BUFFER };
    return {
      ...state,
      hop: HOP_TIME,
      hopBuffer: 0,
      hopCooldown: HOP_COOLDOWN,
      particles: [...state.particles, ...burst(laneX(state.laneVis, OX_ROW), OX_ROW + 3, 4, "dust", 6, 0.3)],
      sfx: pushSfx(state.sfx, "hop"),
    };
  }
  if (key === "ArrowDown") {
    // Fast-fall: cut the hop short to land between two obstacles.
    if (state.hop > 0.08) return { ...state, hop: 0.08 };
    return state;
  }
  return state;
}

/** A new run after a crash: keep the best, and leave the wreck on the trail. */
function restart(state: RunnerState): RunnerState {
  const wreck: Wreck = { lane: Math.round(state.laneVis), row: OX_ROW };
  return resetRunner(state.best, [...state.wrecks, wreck].slice(-3));
}

/** Choose a rider on the attract screen (1/2/3). The choice is a preference,
    so the state-less attract render can show it. */
function attractKey(state: RunnerState, key: string): RunnerState {
  const idx = ["1", "2", "3"].indexOf(key);
  if (idx < 0) return state;
  const rider = RIDERS[idx].id;
  if (!state.unlocked.includes(rider)) return state;
  savePref("dodge-rider", rider);
  return { ...state, rider, sfx: pushSfx(state.sfx, "menu") };
}

/** Taps and swipes: the top third hops, the sides steer, swipes are arrows. */
function runnerPointer(state: RunnerState, p: PointerInput): RunnerState {
  if (state.phase === "dead") return runnerKey(state, "Enter");
  if (p.kind === "tap" && p.y < 1 / 3) return runnerKey(state, "ArrowUp");
  return runnerKey(state, pointerAsKey(p));
}

// ---- spawning --------------------------------------------------------------

function spawnObstacle(s: RunnerState): RunnerState {
  const rising = s.waveT < WAVE_LEN - BREATHER;
  if (!rising) return s;

  const nearHorizon = s.obstacles.filter((o) => o.row < HZ + 5);
  const occupied = new Set(nearHorizon.map((o) => o.lane));
  const open = LANES_BOTTOM.map((_, i) => i).filter((l) => !occupied.has(l));
  // Never block every lane at once: a fresh spawn needs at least two lanes free
  // after it lands (the third stays open as an escape).
  if (open.length < 2) return s;

  // The second ramp: past wave 8 the speed cap holds but bouncers take a
  // bigger share and weeds start arriving in pairs.
  const late = s.wave >= 8;
  const roll = rand();
  let kind: ObstacleKind = "weed";
  if (s.wave >= 2 && roll < 0.14 && nearHorizon.length === 0) kind = "log";
  else if (s.wave >= 3 && roll < (late ? 0.14 + 0.45 : 0.34)) kind = "bouncer";
  else if (roll < (late ? 0.75 : 0.55)) kind = "big";

  const lane = open[Math.floor(rand() * open.length)];
  const make = (k: ObstacleKind, ln: number, row: number): Obstacle => ({
    id: nextId++,
    kind: k,
    lane: k === "log" ? 1 : ln,
    laneVis: k === "log" ? 1 : ln,
    row,
    speed: (0.85 + rand() * 0.3) * (k === "big" ? 0.9 : k === "log" ? 1 : 1.05),
    spin: rand() * Math.PI * 2,
    bounced: false,
    bounceDir: ln === 0 ? 1 : ln === 2 ? -1 : rand() < 0.5 ? -1 : 1,
    scored: false,
    puffed: false,
  });
  const spawned = [make(kind, lane, HZ - 3)];
  // Pair pattern: a second weed one band back in another lane, so exactly one
  // lane stays open and you have to pick it early.
  if (late && kind !== "log" && kind !== "bouncer" && open.length === 3 && rand() < 0.35) {
    const other = open.filter((l) => l !== lane)[Math.floor(rand() * 2)];
    spawned.push(make("weed", other, HZ - 5));
  }

  // Density rises through the wave and across waves, with a hard floor so the
  // gap always leaves a real decision window at top speed. The floor itself
  // tightens over waves 8–14.
  const progress = s.waveT / (WAVE_LEN - BREATHER);
  const floor = 0.42 - 0.08 * clamp((s.wave - 8) / 6, 0, 1);
  const base = Math.max(floor, 1.05 - s.wave * 0.09 - progress * 0.3);
  const gap = (kind === "log" ? base * 1.8 : spawned.length > 1 ? base * 1.4 : base) + rand() * 0.35;
  return { ...s, obstacles: [...s.obstacles, ...spawned], nextSpawn: s.time + gap };
}

function spawnCoin(s: RunnerState): RunnerState {
  const breather = s.waveT >= WAVE_LEN - BREATHER;
  const lane = Math.floor(rand() * 3);
  // Coins come in a short run down one lane — chaining them is the reward.
  const count = breather ? 5 : 3;
  const coins: Pickup[] = [];
  for (let i = 0; i < count; i++) coins.push({ id: nextId++, kind: "coin", lane, row: HZ - 3 - i * 2.2 });
  let next = s;
  if (s.time >= s.nextShieldIn) {
    coins.push({ id: nextId++, kind: "shield", lane: (lane + 1) % 3, row: HZ - 5 });
    next = { ...s, nextShieldIn: s.time + 40 + rand() * 20 };
  }
  return { ...next, pickups: [...s.pickups, ...coins], nextCoin: s.time + (breather ? 1.2 : 3.5 + rand() * 3) };
}

// ---- scoring -------------------------------------------------------------

function bonus(s: RunnerState, x: number, y: number, text: string, pts: number, bump: boolean, big = false): RunnerState {
  const mult = bump ? Math.min(MULT_MAX, s.mult + 1) : s.mult;
  const gained = pts * s.mult * (s.stampede > 0 ? 2 : 1);
  return {
    ...s,
    score: s.score + gained,
    mult,
    maxMult: Math.max(s.maxMult, mult),
    multTimer: MULT_DECAY,
    courage: s.stampede > 0 ? 0 : Math.min(100, s.courage + (bump ? 22 : 8)),
    popups: [...s.popups, { x: x * U, y: y * U, text: text ? `${text} +${gained}` : `+${gained}`, life: 0.9, max: 0.9, big }],
  };
}

// ---- death ----------------------------------------------------------------

function crash(s: RunnerState, ob: Obstacle): RunnerState {
  const x = laneX(s.laneVis, OX_ROW);
  if (s.shield) {
    return {
      ...s,
      shield: false,
      invincible: 0.7,
      obstacles: s.obstacles.filter((o) => o.id !== ob.id),
      hitStop: 0.12 * motionScale(),
      shake: 0.3,
      sfx: pushSfx(s.sfx, "shieldHit"),
      particles: [...s.particles, ...burst(x, OX_ROW, 14, "spark", 18, 0.5), ...burst(x, OX_ROW - 1, 8, "shard", 12, 0.6)],
      popups: [...s.popups, { x: x * U, y: (OX_ROW - 6) * U, text: "SHIELD!", life: 1, max: 1, big: true }],
    };
  }
  const best = saveBest("dodge", s.score);
  return {
    ...s,
    phase: "dead",
    deadFor: 0,
    best,
    hitStop: 0.1 * motionScale(),
    shake: 0.35,
    deathLine: pickRand(DEATH_LINES),
    sfx: pushSfx(s.sfx, "crash"),
    particles: [...s.particles, ...burst(x, OX_ROW, 12, "shard", 16, 0.8), ...burst(x, OX_ROW + 2, 10, "dust", 8, 0.6)],
  };
}

// ---- frame update ---------------------------------------------------------

function runnerUpdate(state: RunnerState, rawDt: number): RunnerState {
  let s = state;

  // Hit-stop freezes the world (but not the juice timers) for a few frames.
  if (s.hitStop > 0) return { ...s, hitStop: s.hitStop - rawDt, shake: Math.max(0, s.shake - rawDt) };

  if (s.phase === "dead") {
    return {
      ...s,
      deadFor: s.deadFor + rawDt,
      shake: Math.max(0, s.shake - rawDt),
      particles: stepParticles(s.particles, rawDt, 40),
      popups: stepPopups(s.popups, rawDt),
    };
  }

  const dt = s.slowMo > 0 ? rawDt * 0.5 : rawDt;
  const time = s.time + rawDt;
  const phase: Phase = s.phase === "ready" && time >= READY_TIME ? "run" : s.phase;
  // Soft start: the trail rolls up to speed over the first second.
  const ramp = phase === "ready" ? 0.4 + (0.6 * time) / READY_TIME : 1;
  const speed = speedFor(s.wave) * ramp;

  let waveT = s.waveT + dt;
  let wave = s.wave;
  if (waveT >= WAVE_LEN) {
    waveT -= WAVE_LEN;
    wave += 1;
  }

  // Lane tween with an input buffer.
  let laneVis = s.laneVis;
  let oxLane = s.oxLane;
  let queuedLane = s.queuedLane;
  let squash = Math.max(0, s.squash - dt);
  const diff = oxLane - laneVis;
  if (Math.abs(diff) > 0.001) {
    const step = dt / LANE_TWEEN;
    laneVis = Math.abs(diff) <= step ? oxLane : laneVis + Math.sign(diff) * step;
    if (laneVis === oxLane) {
      squash = 0.08;
      if (queuedLane !== null) {
        oxLane = queuedLane;
        queuedLane = null;
      }
    }
  }

  const hop = Math.max(0, s.hop - dt);
  const landed = s.hop > 0 && hop === 0;
  const hopCooldown = Math.max(0, s.hopCooldown - dt);

  const dist = s.dist + dt * speed;
  let particles = stepParticles(s.particles, dt, 40);
  let sfx = s.sfx;
  if (landed) {
    particles = [...particles, ...burst(laneX(laneVis, OX_ROW), OX_ROW + 3, 6, "dust", 7, 0.35)];
    squash = 0.1;
    sfx = pushSfx(sfx, "land");
  }

  // Multiplier decays a step at a time when you play it safe.
  let mult = s.mult;
  let multTimer = s.multTimer - dt;
  if (multTimer <= 0) {
    mult = Math.max(1, mult - 1);
    multTimer = MULT_DECAY;
  }

  // Distance points, scaled by the multiplier — keeping it high is the game.
  const score = s.score + dt * 10 * mult * (s.stampede > 0 ? 2 : 1) * (phase === "run" ? 1 : 0);

  s = {
    ...s,
    phase,
    time,
    waveT,
    wave,
    laneVis,
    oxLane,
    queuedLane,
    squash,
    hop,
    hopCooldown,
    hopBuffer: Math.max(0, s.hopBuffer - dt),
    invincible: s.stampede > 0 && s.stampede <= dt ? 0.7 : Math.max(0, s.invincible - dt),
    stampede: Math.max(0, s.stampede - dt),
    dist,
    particles,
    sfx,
    mult,
    multTimer,
    score,
    wrecks: s.wrecks.map((w) => ({ ...w, row: w.row + speed * dt })).filter((w) => w.row < ROWS + 4),
    slowMo: Math.max(0, s.slowMo - rawDt),
    shake: Math.max(0, s.shake - rawDt),
    popups: stepPopups(s.popups, rawDt),
    banner: s.banner && s.banner.life - rawDt > 0 ? { ...s.banner, life: s.banner.life - rawDt } : null,
  };

  if (s.hopBuffer > 0 && s.hopCooldown === 0 && s.hop === 0) s = runnerKey(s, "ArrowUp");

  // Move obstacles; bouncers wobble, puff dust as a tell, then hop one lane at
  // BOUNCE_ROW.
  let tellPuffs: Particle[] = [];
  let obstacles = s.obstacles.map((o) => {
    const row = o.row + o.speed * speed * dt;
    let lane = o.lane;
    let bounced = o.bounced;
    let puffed = o.puffed;
    let laneVisNext = o.laneVis;
    if (o.kind === "bouncer" && !puffed && row >= BOUNCE_ROW - 3) {
      puffed = true;
      tellPuffs = [...tellPuffs, ...burst(laneX(o.laneVis, row) + o.bounceDir * 2, row, 3, "dust", 5, 0.4)];
    }
    if (o.kind === "bouncer" && !bounced && row >= BOUNCE_ROW) {
      lane = clamp(o.lane + o.bounceDir, 0, 2);
      bounced = true;
    }
    if (laneVisNext !== lane) {
      const step = dt / 0.25;
      laneVisNext = Math.abs(lane - laneVisNext) <= step ? lane : laneVisNext + Math.sign(lane - laneVisNext) * step;
    }
    return { ...o, row, lane, laneVis: laneVisNext, bounced, puffed, spin: o.spin + dt * 5 };
  });
  if (tellPuffs.length) s = { ...s, particles: [...s.particles, ...tellPuffs] };
  let pickups = s.pickups.map((p) => ({ ...p, row: p.row + speed * dt }));

  // Spawns.
  if (phase === "run" && s.time >= s.nextSpawn) s = spawnObstacle({ ...s, obstacles });
  else s = { ...s, obstacles };
  if (phase === "run" && s.time >= s.nextCoin) s = spawnCoin({ ...s, pickups });
  else s = { ...s, pickups };
  obstacles = s.obstacles;
  pickups = s.pickups;

  // Pickups: coins chain toward a multiplier bump; horseshoes grant a shield.
  const oxX = laneX(s.laneVis, OX_ROW);
  for (const p of pickups) {
    const near = (s.stampede > 0 || Math.abs(p.lane - s.laneVis) < 0.5) && Math.abs(p.row - OX_ROW) < 1.8;
    if (!near) continue;
    pickups = pickups.filter((q) => q.id !== p.id);
    if (p.kind === "shield") {
      s = { ...s, shield: true, popups: [...s.popups, { x: oxX * U, y: (OX_ROW - 6) * U, text: "HORSESHOE!", life: 1, max: 1, big: true }] };
      s = { ...s, particles: [...s.particles, ...burst(oxX, OX_ROW, 10, "spark", 12, 0.5)], sfx: pushSfx(s.sfx, "shield") };
    } else {
      const chain = s.coinChain + 1;
      const bump = chain % 5 === 0;
      s = bonus({ ...s, coinChain: chain }, oxX, OX_ROW - 5, bump ? "CHAIN" : "", 10, bump);
      s = { ...s, particles: [...s.particles, ...burst(oxX, OX_ROW - 1, 3, "spark", 8, 0.3)], sfx: pushSfx(s.sfx, bump ? "chain" : "coin") };
    }
  }
  pickups = pickups.filter((p) => p.row < ROWS + 2);

  if (s.courage >= 100 && s.stampede === 0) {
    s = { ...s, courage: 0, stampede: STAMPEDE_SECONDS,
      banner: { text: "STAMPEDE · SMASH + DOUBLE POINTS", life: 1.5 },
      sfx: pushSfx(s.sfx, "milestone"),
      particles: [...s.particles, ...burst(oxX, OX_ROW, 18, "spark", 18, 0.7)] };
  }

  // Obstacles: collide, graze, or hop.
  const airborne = s.hop > 0;
  for (const o of obstacles) {
    const dRow = o.row - OX_ROW;
    const laneGap = Math.abs(o.laneVis - s.laneVis);
    const hitRows = o.kind === "log" ? 1.8 : o.kind === "big" ? 3 : 2.6;
    const inLane = o.kind === "log" ? true : laneGap < 0.55;

    if (inLane && Math.abs(dRow) < hitRows) {
      if (s.stampede > 0) {
        obstacles = obstacles.filter((q) => q.id !== o.id);
        s = bonus(s, oxX, OX_ROW - 7, "SMASH", 30, false);
        s = { ...s, particles: [...s.particles, ...burst(oxX, o.row, 8, "shard", 16, 0.5)], sfx: pushSfx(s.sfx, "chain") };
        continue;
      }
      if (s.invincible > 0) continue;
      if (!airborne) return crash({ ...s, obstacles, pickups }, o);
      // Passing overhead: reward once per obstacle.
      if (!o.scored) {
        obstacles = obstacles.map((q) => (q.id === o.id ? { ...q, scored: true } : q));
        s = bonus(s, oxX, OX_ROW - 7, o.kind === "log" ? "CLEARED" : "HOPPED", o.kind === "log" ? 20 : 25, true);
      }
      continue;
    }
    // Graze: it rolled past in the neighbouring lane. The riskier the closer.
    if (!o.scored && o.kind !== "log" && laneGap >= 0.55 && laneGap < 1.4 && dRow > 0 && dRow < 1.2) {
      obstacles = obstacles.map((q) => (q.id === o.id ? { ...q, scored: true } : q));
      s = bonus(s, oxX, OX_ROW - 7, "CLOSE!", 25, true);
      s = { ...s, particles: [...s.particles, ...burst(laneX(o.laneVis, o.row), o.row, 4, "dust", 6, 0.3)], sfx: pushSfx(s.sfx, "graze") };
    }
  }
  obstacles = obstacles.filter((o) => o.row < ROWS + 4);

  // Milestones: a landmark banner, a beat of slow-mo, and a fresh coin run.
  let milestone = s.milestone;
  let banner = s.banner;
  let slowMo = s.slowMo;
  let coinChain = s.coinChain;
  let sfxOut = s.sfx;
  let unlocked = s.unlocked;
  let popupsExtra: Popup[] = [];
  if (s.dist >= (milestone + 1) * LANDMARK_EVERY) {
    milestone += 1;
    const name = LANDMARKS[milestone - 1] || `${milestone * LANDMARK_EVERY} yards`;
    banner = { text: name.toUpperCase(), life: 1.6 };
    slowMo = 0.15;
    sfxOut = pushSfx(sfxOut, "milestone");
  }
  // Riders unlock by distance in a single run and stay unlocked for good.
  for (const r of RIDERS) {
    if (r.yards > 0 && s.dist >= r.yards && !unlocked.includes(r.id)) {
      unlocked = [...unlocked, r.id];
      savePref("dodge-unlocks", unlocked.join(","));
      popupsExtra.push({ x: (COLS * U) / 2, y: (HZ + 8) * U, text: `${r.label} UNLOCKED!`, life: 1.6, max: 1.6, big: true });
      sfxOut = pushSfx(sfxOut, "best");
    }
  }
  // Chains break if you let a coin roll by.
  if (pickups.some((p) => p.kind === "coin" && p.row > OX_ROW + 2 && Math.abs(p.lane - s.laneVis) >= 0.5) && s.coinChain > 0) {
    coinChain = 0;
    pickups = pickups.filter((p) => !(p.kind === "coin" && p.row > OX_ROW + 2));
  }

  const newBest = s.best > 0 && s.score > s.best;
  const justBeat = newBest && !s.newBest;
  let popups = [...s.popups, ...popupsExtra];
  if (justBeat) {
    popups = [...popups, { x: (COLS * U) / 2, y: (HZ + 4) * U, text: "NEW BEST!", life: 1.4, max: 1.4, big: true }];
    particles = [...s.particles, ...burst(COLS / 2, HZ + 4, motionScale() < 1 ? 8 : 16, "spark", 14, 0.9)];
    sfxOut = pushSfx(sfxOut, "best");
  } else particles = s.particles;

  return { ...s, obstacles, pickups, milestone, banner, slowMo, coinChain, newBest, popups, particles, sfx: sfxOut, unlocked };
}

// ---- rendering --------------------------------------------------------------

/** Sky, sun, mountains, prairie, and the converging trail — shared by the
    attract screen and the live game. `dist` scrolls the ruts and tufts. */
function Prairie({ c, dist }: { c: SceneColors; dist: number }) {
  const scroll = Math.floor(dist);

  // Wheel ruts: chunky dashes marching down the two lane boundaries.
  const ruts: React.JSX.Element[] = [];
  RUTS_BOTTOM.forEach((bottom, i) => {
    for (let row = HZ + 1; row < ROWS; row++) {
      if ((row + scroll) % 3 !== 0) continue;
      const t = (row - HZ) / (ROWS - HZ);
      const x = Math.round(RUTS_HORIZON[i] + (bottom - RUTS_HORIZON[i]) * t);
      ruts.push(<Px key={`r${i}-${row}`} x={x} y={row} w={1} h={1} fill={c.line} o={0.55} />);
    }
  });

  // Roadside grass tufts drifting past to sell the speed.
  const tuftXs = [4, 66, 8, 61, 2, 68];
  const span = ROWS - HZ + 3;
  const tufts = tuftXs.map((x, k) => {
    const row = HZ + 1 + ((k * 5 + scroll) % span);
    if (row >= ROWS) return null;
    return (
      <g key={`t${k}`}>
        <Px x={x} y={row} w={2} h={1} fill={c.sky} o={0.4} />
        <Px x={x + (k % 2)} y={row - 1} w={1} h={1} fill={c.sky} o={0.4} />
      </g>
    );
  });

  return (
    <g>
      <Vista c={c} horizon={HZ} travel={dist} />
      <Pine x={6} y={19} c={c} scale={1.1} />
      <Pine x={66} y={26} c={c} scale={1.5} />
      {poly([[29, HZ], [43, HZ], [59, ROWS], [13, ROWS]], c.trail, 0.5)}
      {poly([[29, HZ], [30, HZ], [15, ROWS], [13, ROWS]], c.sun, 0.35)}
      {poly([[42, HZ], [43, HZ], [59, ROWS], [57, ROWS]], c.sky, 0.3)}
      {ruts}
      {tufts}
    </g>
  );
}

/** Day rolls into dusk, night, and dawn as the yards pile up, so a glance at
    the sky says how far you got. Stars only come out in the dark. */
function DayCycle({ dist }: { dist: number }) {
  const t = (dist / 2400) % 1;
  const dark = Math.max(0, Math.sin(Math.PI * t)) ** 2; // 0 at day, 1 at midnight
  if (dark < 0.02) return null;
  const stars: [number, number][] = [[6, 3], [14, 6], [22, 2], [40, 4], [50, 2], [66, 5], [70, 1], [33, 7]];
  return (
    <g>
      <Px x={0} y={0} w={COLS} h={HZ} fill="#0a0820" o={dark * 0.7} />
      <Px x={0} y={HZ} w={COLS} h={ROWS - HZ} fill="#050510" o={dark * 0.45} />
      {dark > 0.45 && stars.map(([x, y], i) => <Px key={i} x={x} y={y} w={1} h={1} fill="#fff" o={(dark - 0.45) * 1.5 * (i % 2 ? 0.6 : 1)} />)}
    </g>
  );
}

/** The ox, with a two-frame gallop. `step` toggles which legs are planted. */
function OxSprite({ x, y, step, fill, shadow, lean = 0, squash = 0, spin = 0, lift = 0 }: { x: number; y: number; step: number; fill: string; shadow: string; lean?: number; squash?: number; spin?: number; lift?: number }) {
  const px = Math.round(x);
  const py = Math.round(y);
  const sy = 1 - squash;
  const sx = 1 + squash * 0.6;
  const transforms = [
    `translate(${(px - 5) * U} ${(py - 3 - lift) * U})`,
    ...(spin ? [`rotate(${spin} ${5 * U} ${4 * U})`] : []),
    ...(lean ? [`skewX(${-lean * 14})`] : []),
    ...(squash ? [`translate(${5 * U} ${7 * U}) scale(${sx} ${sy}) translate(${-5 * U} ${-7 * U})`] : []),
  ];
  return (
    <g>
      {lift > 0 && <Px x={px - 4} y={py + 3} w={9} h={1} fill={shadow} o={0.25} />}
      <g transform={transforms.join(" ")}>
        {lift === 0 && <Px x={1} y={3} w={9} h={4} fill={shadow} o={0.3} />}
        {/* horns — light base, dark keratin tips curling up and out */}
        <Px x={0} y={0} w={2} h={1} fill={fill} />
        <Px x={3} y={0} w={1} h={1} fill={fill} />
        <Px x={-1} y={0} w={1} h={1} fill={fill} />
        <Px x={4} y={0} w={1} h={1} fill={fill} />
        <Px x={-2} y={-1} w={1} h={1} fill={shadow} />
        <Px x={5} y={-1} w={1} h={1} fill={shadow} />
        {/* head, eye */}
        <Px x={0} y={1} w={4} h={3} fill={fill} />
        <Px x={1} y={2} w={1} h={1} fill={shadow} />
        {/* body and tail */}
        <Px x={3} y={1} w={7} h={4} fill={fill} />
        <Px x={3} y={4} w={7} h={1} fill={shadow} o={0.22} />
        <Px x={8} y={2} w={2} h={2} fill={shadow} o={0.12} />
        <Px x={3} y={1} w={1} h={3} fill="#bb563f" />
        <Px x={4} y={2} w={1.5} h={0.5} fill="#f0be8c" />
        <Px x={0} y={3} w={2} h={1} fill={shadow} o={0.28} />
        <Px x={1.5} y={2} w={0.5} h={0.5} fill="#fff7db" />
        <Px x={10} y={2} w={1} h={1} fill={fill} />
        {/* galloping legs (tucked while airborne) */}
        {lift > 0 ? (
          <g>
            <Px x={4} y={5} w={2} h={1} fill={shadow} />
            <Px x={8} y={5} w={2} h={1} fill={shadow} />
          </g>
        ) : step === 0 ? (
          <g>
            <Px x={4} y={5} w={1} h={2} fill={shadow} />
            <Px x={8} y={5} w={1} h={2} fill={shadow} />
          </g>
        ) : (
          <g>
            <Px x={5} y={5} w={1} h={2} fill={shadow} />
            <Px x={7} y={5} w={1} h={2} fill={shadow} />
            <Px x={10} y={6} w={1} h={1} fill={shadow} o={0.4} />
          </g>
        )}
      </g>
    </g>
  );
}

/** The mule: long ears, a thinner grey body, same gallop frames. */
function MuleSprite({ x, y, step, fill, shadow, lean = 0, squash = 0, spin = 0, lift = 0 }: { x: number; y: number; step: number; fill: string; shadow: string; lean?: number; squash?: number; spin?: number; lift?: number }) {
  const px = Math.round(x);
  const py = Math.round(y);
  const sy = 1 - squash;
  const sx = 1 + squash * 0.6;
  const transforms = [
    `translate(${(px - 5) * U} ${(py - 3 - lift) * U})`,
    ...(spin ? [`rotate(${spin} ${5 * U} ${4 * U})`] : []),
    ...(lean ? [`skewX(${-lean * 14})`] : []),
    ...(squash ? [`translate(${5 * U} ${7 * U}) scale(${sx} ${sy}) translate(${-5 * U} ${-7 * U})`] : []),
  ];
  return (
    <g>
      {lift > 0 && <Px x={px - 4} y={py + 3} w={9} h={1} fill={shadow} o={0.25} />}
      <g transform={transforms.join(" ")} opacity={0.92}>
        {lift === 0 && <Px x={1} y={3} w={8} h={4} fill={shadow} o={0.3} />}
        {/* long ears */}
        <Px x={0} y={-1} w={1} h={2} fill={fill} />
        <Px x={2} y={-1} w={1} h={2} fill={fill} />
        {/* head, eye, muzzle */}
        <Px x={0} y={1} w={3} h={2} fill={fill} />
        <Px x={-1} y={2} w={1} h={1} fill={fill} />
        <Px x={1} y={1} w={1} h={1} fill={shadow} />
        {/* thin body and tail */}
        <Px x={3} y={2} w={6} h={3} fill={fill} />
        <Px x={9} y={2} w={1} h={2} fill={shadow} />
        {lift > 0 ? (
          <g>
            <Px x={4} y={5} w={2} h={1} fill={shadow} />
            <Px x={7} y={5} w={2} h={1} fill={shadow} />
          </g>
        ) : step === 0 ? (
          <g>
            <Px x={4} y={5} w={1} h={2} fill={shadow} />
            <Px x={8} y={5} w={1} h={2} fill={shadow} />
          </g>
        ) : (
          <g>
            <Px x={5} y={5} w={1} h={2} fill={shadow} />
            <Px x={7} y={5} w={1} h={2} fill={shadow} />
          </g>
        )}
      </g>
    </g>
  );
}

/** The wagon rider: a lane-sized covered wagon that bounces instead of galloping. */
function WagonRiderSprite({ x, y, step, canvas, wood, dark, lean = 0, squash = 0, spin = 0, lift = 0 }: { x: number; y: number; step: number; canvas: string; wood: string; dark: string; lean?: number; squash?: number; spin?: number; lift?: number }) {
  const px = Math.round(x);
  const py = Math.round(y);
  const bob = step === 0 ? 0 : -1;
  const sy = 1 - squash;
  const sx = 1 + squash * 0.6;
  const transforms = [
    `translate(${(px - 5) * U} ${(py - 3 - lift + bob) * U})`,
    ...(spin ? [`rotate(${spin} ${5 * U} ${4 * U})`] : []),
    ...(lean ? [`skewX(${-lean * 10})`] : []),
    ...(squash ? [`translate(${5 * U} ${7 * U}) scale(${sx} ${sy}) translate(${-5 * U} ${-7 * U})`] : []),
  ];
  const w = 9;
  const bars = [];
  for (let i = 0; i <= w; i++) {
    const top = 3 - Math.round(3 * Math.sin((Math.PI * i) / w));
    bars.push(<Px key={i} x={i} y={top} w={1} h={3 - top + 1} fill={canvas} />);
  }
  return (
    <g>
      {lift > 0 && <Px x={px - 4} y={py + 3} w={9} h={1} fill={dark} o={0.25} />}
      <g transform={transforms.join(" ")}>
        {lift === 0 && <Px x={1} y={3} w={9} h={4} fill={dark} o={0.3} />}
        {bars}
        <Px x={0} y={4} w={w + 1} h={2} fill={wood} />
        <Px x={1} y={5} w={2} h={2} fill={dark} />
        <Px x={7} y={5} w={2} h={2} fill={dark} />
        <Px x={1 + (step ? 1 : 0)} y={5} w={1} h={1} fill={canvas} o={0.6} />
        <Px x={7 + (step ? 1 : 0)} y={5} w={1} h={1} fill={canvas} o={0.6} />
      </g>
    </g>
  );
}

/** What a crash leaves behind: half a weed and the hat that flew off. */
function WreckSprite({ x, y, fill, dark }: { x: number; y: number; fill: string; dark: string }) {
  const px = Math.round(x);
  const py = Math.round(y);
  const half: [number, number][] = [[-3, 0], [3, 0], [-3, 1], [3, 1], [-2, 2], [2, 2], [-1, 3], [0, 3], [1, 3], [0, 1]];
  return (
    <g opacity={0.85}>
      {half.map(([dx, dy]) => (
        <Px key={`${dx},${dy}`} x={px + dx} y={py + dy} w={1} h={1} fill={fill} />
      ))}
      <Px x={px + 4} y={py + 2} w={2} h={1} fill={dark} />
      <Px x={px + 3} y={py + 3} w={4} h={1} fill={dark} o={0.7} />
    </g>
  );
}

// Tumbleweed sprites: a pixel ring with spokes that alternate between an ×
// and a + as it rolls — the 8-bit stand-in for rotation.
const WEED_RING_BIG: [number, number][] = [
  [-1, -3], [0, -3], [1, -3],
  [-2, -2], [2, -2],
  [-3, -1], [3, -1],
  [-3, 0], [3, 0],
  [-3, 1], [3, 1],
  [-2, 2], [2, 2],
  [-1, 3], [0, 3], [1, 3],
];
const WEED_SPOKES_X: [number, number][] = [[-1, -1], [1, 1], [-1, 1], [1, -1]];
const WEED_SPOKES_PLUS: [number, number][] = [[0, -1], [0, 1], [-1, 0], [1, 0]];
const WEED_RING_SMALL: [number, number][] = [
  [-1, -2], [0, -2], [1, -2],
  [-2, -1], [2, -1],
  [-2, 0], [2, 0],
  [-2, 1], [2, 1],
  [-1, 2], [0, 2], [1, 2],
];
const WEED_RING_HUGE: [number, number][] = [
  [-1, -4], [0, -4], [1, -4],
  [-3, -3], [-2, -3], [2, -3], [3, -3],
  [-4, -1], [-4, -2], [4, -1], [4, -2],
  [-4, 0], [4, 0],
  [-4, 1], [-4, 2], [4, 1], [4, 2],
  [-3, 3], [-2, 3], [2, 3], [3, 3],
  [-1, 4], [0, 4], [1, 4],
];

function WeedSprite({ x, y, spin, size, fill, dark, wobble, tell }: { x: number; y: number; spin: number; size: "small" | "mid" | "huge"; fill: string; dark: string; wobble?: boolean; tell?: number }) {
  const px = Math.round(x + (wobble ? Math.round(Math.sin(spin * 6) * 2) : 0));
  const py = Math.round(y);
  const frame = Math.floor(spin * 2) % 2;
  const ring = size === "small" ? WEED_RING_SMALL : size === "huge" ? WEED_RING_HUGE : WEED_RING_BIG;
  const spokes = size === "small" ? [] : frame === 0 ? WEED_SPOKES_X : WEED_SPOKES_PLUS;
  return (
    <g>
      {ring.map(([dx, dy]) => (
        <Px key={`${dx},${dy}`} x={px + dx} y={py + dy} w={1} h={1} fill={fill} />
      ))}
      {spokes.map(([dx, dy]) => (
        <Px key={`s${dx},${dy}`} x={px + dx} y={py + dy} w={1} h={1} fill={fill} o={0.75} />
      ))}
      <Px x={px} y={py} w={1} h={1} fill={dark} o={0.6} />
      {wobble && <Px x={px} y={py - (size === "small" ? 4 : 5)} w={1} h={1} fill={dark} o={0.8} />}
      {/* the tell: a chevron beside a bouncer pointing where it will jump */}
      {tell ? (
        <g>
          <Px x={px + tell * (size === "small" ? 4 : 5)} y={py} w={1} h={1} fill={dark} />
          <Px x={px + tell * (size === "small" ? 3 : 4)} y={py - 1} w={1} h={1} fill={dark} />
          <Px x={px + tell * (size === "small" ? 3 : 4)} y={py + 1} w={1} h={1} fill={dark} />
        </g>
      ) : null}
    </g>
  );
}

/** A fallen log across the whole trail — the one thing you must hop. */
function LogSprite({ row, fill, dark }: { row: number; fill: string; dark: string }) {
  const y = Math.round(row);
  const x0 = Math.round(laneX(0, row)) - 4;
  const x1 = Math.round(laneX(2, row)) + 4;
  return (
    <g>
      <Px x={x0} y={y - 1} w={x1 - x0} h={2} fill={fill} />
      <Px x={x0} y={y + 1} w={x1 - x0} h={1} fill={dark} o={0.5} />
      <Px x={x0 - 1} y={y} w={1} h={1} fill={dark} />
      <Px x={x1} y={y} w={1} h={1} fill={dark} />
      {Array.from({ length: Math.max(1, Math.floor((x1 - x0) / 5)) }, (_, i) => (
        <Px key={i} x={x0 + 2 + i * 5} y={y - 1} w={1} h={1} fill={dark} o={0.5} />
      ))}
    </g>
  );
}

function CoinSprite({ x, y, t, fill, bright }: { x: number; y: number; t: number; fill: string; bright: string }) {
  const px = Math.round(x);
  const py = Math.round(y);
  const glint = Math.floor(t * 4) % 3 === 0;
  return (
    <g>
      <Px x={px - 1} y={py - 1} w={2} h={2} fill={fill} />
      <Px x={px - 1} y={py - 1} w={1} h={1} fill={glint ? bright : fill} />
    </g>
  );
}

function HorseshoeSprite({ x, y, fill, bright }: { x: number; y: number; fill: string; bright: string }) {
  const px = Math.round(x);
  const py = Math.round(y);
  return (
    <g>
      <Px x={px - 2} y={py - 2} w={1} h={3} fill={fill} />
      <Px x={px + 1} y={py - 2} w={1} h={3} fill={fill} />
      <Px x={px - 1} y={py + 1} w={2} h={1} fill={fill} />
      <Px x={px - 2} y={py - 2} w={1} h={1} fill={bright} />
    </g>
  );
}

/** The covered wagon from the old trail scene, for the attract screen. */
function WagonSprite({ x, y, canvas, wood, dark }: { x: number; y: number; canvas: string; wood: string; dark: string }) {
  const w = 14;
  const domeBase = 6;
  const bars = [];
  for (let i = 0; i <= w; i++) {
    const top = domeBase - Math.round(5 * Math.sin((Math.PI * i) / w));
    bars.push(<Px key={i} x={x + i} y={y + top} w={1} h={domeBase - top} fill={canvas} />);
  }
  return (
    <g>
      {bars}
      <Px x={x} y={y + domeBase} w={w + 1} h={3} fill={wood} />
      <Px x={x + 2} y={y + domeBase + 2} w={4} h={4} fill={canvas} />
      <Px x={x + 3} y={y + domeBase + 3} w={2} h={2} fill={dark} />
      <Px x={x + 9} y={y + domeBase + 2} w={4} h={4} fill={canvas} />
      <Px x={x + 10} y={y + domeBase + 3} w={2} h={2} fill={dark} />
    </g>
  );
}

const pad4 = (n: number) => Math.floor(n).toString().padStart(4, "0");

function Hud({ s, c }: { s: RunnerState; c: SceneColors }) {
  const W = COLS * U;
  const onFire = s.mult >= MULT_MAX;
  const hot = s.mult >= 4;
  const decay = clamp(s.multTimer / MULT_DECAY, 0, 1);
  const multColor = onFire ? c.danger : hot ? c.accent : c.text;
  return (
    <g>
      <PxText x={8} y={17} size={11} fill={c.text} shadow={c.sky}>{pad4(s.score)}</PxText>
      <PxText x={8} y={27} size={7} fill={s.newBest ? c.accent : c.text} shadow={c.sky}>
        {s.newBest ? "NEW BEST" : `BEST ${pad4(s.best)}`}
      </PxText>
      {/* the chase: a meter that fills toward BEST, with a tick at the line */}
      {s.best > 0 && (
        <g>
          <rect x={8} y={30} width={60} height={2} fill={c.sky} opacity={0.6} />
          <rect x={8} y={30} width={60 * Math.min(1, s.score / s.best)} height={2} fill={s.newBest ? c.accent : c.text} opacity={0.9} />
          <rect x={67} y={29} width={1} height={4} fill={s.newBest ? c.accent : c.snow} />
        </g>
      )}
      <rect x={W / 2 - 39} y={7} width={78} height={18} fill={c.sky} opacity={0.85} />
      <PxText x={W / 2} y={15} size={6} fill={c.accent} shadow={c.sky} anchor="middle">
        {s.stampede > 0 ? `STAMPEDE ${Math.ceil(s.stampede)}s · x2` : "BUILD YOUR STAMPEDE"}
      </PxText>
      {Array.from({ length: 10 }, (_, i) => <rect key={i} x={W / 2 - 34 + i * 7} y={19} width={5} height={3} fill={c.accent} opacity={i < (s.stampede > 0 ? s.stampede / STAMPEDE_SECONDS : s.courage / 100) * 10 ? 1 : 0.2} />)}
      {/* multiplier with its decay bar */}
      <PxText x={W - 8} y={17} size={11} fill={multColor} shadow={c.sky} anchor="end">{`x${s.mult}`}</PxText>
      <rect x={W - 8 - 22} y={20} width={22} height={2} fill={c.sky} opacity={0.6} />
      <rect x={W - 8 - 22 * decay} y={20} width={22 * decay} height={2} fill={multColor} />
      {onFire && (
        <PxText x={W - 8} y={30} size={7} fill={c.danger} shadow={c.sky} anchor="end">
          {Math.floor(s.time * 6) % 2 === 0 ? "ON FIRE" : "ON FIRE!"}
        </PxText>
      )}
      {s.shield && <HorseshoeSprite x={COLS - 6} y={ROWS - 3} fill={c.accent} bright={c.snow} />}
      {s.hopCooldown > 0 && s.phase === "run" && (
        <rect x={8} y={(ROWS - 3) * U} width={14 * (1 - s.hopCooldown / HOP_COOLDOWN)} height={2} fill={c.text} opacity={0.5} />
      )}
    </g>
  );
}

/** Whichever rider is chosen, drawn with the shared gallop/hop/tumble args. */
function RiderSprite({ rider, c, x, y, step, lean, squash, spin, lift, tint }: { rider: Rider; c: SceneColors; x: number; y: number; step: number; lean?: number; squash?: number; spin?: number; lift?: number; tint?: string }) {
  if (rider === "mule") return <MuleSprite x={x} y={y} step={step} fill={tint ?? c.mountainBack} shadow={c.line} lean={lean} squash={squash} spin={spin} lift={lift} />;
  if (rider === "wagon") return <WagonRiderSprite x={x} y={y} step={step} canvas={tint ?? c.snow} wood={c.mountain} dark={c.line} lean={lean} squash={squash} spin={spin} lift={lift} />;
  return <OxSprite x={x} y={y} step={step} fill={tint ?? c.ox} shadow={c.line} lean={lean} squash={squash} spin={spin} lift={lift} />;
}

function RunnerRender(state: RunnerState, p: ThemePalette) {
  const c = sceneColors(p);
  const s = state;
  const W = COLS * U;
  const oxStep = Math.floor(s.dist * 0.6) % 2;
  const lean = clamp((s.oxLane - s.laneVis) * 2, -1, 1);
  const lift = s.hop > 0 ? Math.sin((Math.PI * (HOP_TIME - s.hop)) / HOP_TIME) * HOP_HEIGHT : 0;
  const spin = s.phase === "dead" ? Math.min(360, s.deadFor * 720) : 0;
  const dead = s.phase === "dead";
  const onFire = s.mult >= MULT_MAX;
  const hot = s.mult >= 4;

  return (
    <GameFrame label="Tumbleweed Dodge: steer the ox around tumbleweeds rolling down the trail">
      <g transform={shakeTransform(s.shake, 3)}>
        <Prairie c={c} dist={s.dist} />
        <DayCycle dist={s.dist} />
        {s.stampede > 0 && <g opacity={0.5 * motionScale()}>
          {Array.from({ length: 12 }, (_, i) => <Px key={i} x={i % 2 ? 3 + i : COLS - 3 - i} y={HZ + (i * 5 + Math.floor(s.time * 25)) % (ROWS - HZ)} w={0.5} h={3} fill={c.accent} />)}
          <rect x={2} y={2} width={W - 4} height={ROWS * U - 4} fill="none" stroke={c.accent} strokeWidth={2} />
        </g>}
        {s.obstacles.filter((o) => o.kind === "log" && o.row < OX_ROW - 6).map((o) => <PxText key={`tell${o.id}`} x={W / 2} y={52} size={7} fill={c.accent} shadow={c.sky} anchor="middle">↑ HOP THE LOG</PxText>)}
        {onFire && <rect x={0} y={0} width={W} height={ROWS * U} fill={c.danger} opacity={(0.06 + 0.04 * Math.sin(s.time * 12)) * motionScale()} />}
        {s.wrecks.map((w, i) =>
          w.row < HZ ? null : <WreckSprite key={`w${i}`} x={laneX(w.lane, w.row)} y={w.row} fill={c.weed} dark={c.line} />,
        )}
        {s.pickups.map((pk) =>
          pk.row < HZ ? null : pk.kind === "coin" ? (
            <CoinSprite key={pk.id} x={laneX(pk.lane, pk.row)} y={pk.row} t={s.time} fill={c.sun} bright={c.snow} />
          ) : (
            <HorseshoeSprite key={pk.id} x={laneX(pk.lane, pk.row)} y={pk.row} fill={c.accent} bright={c.snow} />
          ),
        )}
        {s.obstacles.map((o) =>
          o.row < HZ ? null : o.kind === "log" ? (
            <LogSprite key={o.id} row={o.row} fill={c.mountain} dark={c.sky} />
          ) : (
            <WeedSprite
              key={o.id}
              x={laneX(o.laneVis, o.row)}
              y={o.row}
              spin={o.spin}
              size={o.row < HZ + 8 ? "small" : o.kind === "big" ? "huge" : "mid"}
              fill={c.weed}
              dark={c.sky}
              wobble={o.kind === "bouncer" && !o.bounced}
              tell={o.kind === "bouncer" && !o.bounced ? o.bounceDir : 0}
            />
          ),
        )}
        {/* afterimage trail when the combo is hot */}
        {hot && !dead && s.hop === 0 && (
          <g opacity={0.25}>
            <RiderSprite rider={s.rider} c={c} x={laneX(s.laneVis, OX_ROW) + 0.5} y={OX_ROW + 1.5} step={1 - oxStep} tint={onFire ? c.danger : c.accent} />
          </g>
        )}
        <RiderSprite rider={s.rider} c={c} tint={s.stampede > 0 || s.invincible > 0 ? c.sun : undefined} x={laneX(s.laneVis, OX_ROW)} y={OX_ROW} step={dead ? 0 : oxStep} lean={dead ? 0 : lean} squash={s.squash > 0 ? 0.18 : 0} spin={spin} lift={lift} />
        {s.shield && !dead && (
          <g opacity={0.5 + 0.3 * Math.sin(s.time * 10)}>
            <rect x={(laneX(s.laneVis, OX_ROW) - 7) * U} y={(OX_ROW - 5 - lift) * U} width={14 * U} height={10 * U} fill="none" stroke={c.accent} strokeWidth={U / 2} />
          </g>
        )}
        <Particles ps={s.particles} fill={c.weed} spark={c.sun} />
        <Popups ps={s.popups} fill={c.snow} shadow={c.sky} />
        <Hud s={s} c={c} />
        {s.banner && (
          <g opacity={Math.min(1, s.banner.life * 2)}>
            <Px x={0} y={HZ + 2} w={COLS} h={5} fill={c.sky} o={0.55} />
            <PxText x={W / 2} y={(HZ + 5.6) * U} size={11} fill={c.accent} shadow="#000" anchor="middle">{s.banner.text}</PxText>
          </g>
        )}
        {s.phase === "ready" && (
          <PxText x={W / 2} y={(HZ + 6) * U} size={14} fill={c.snow} shadow="#000" anchor="middle">
            {s.time < READY_TIME * 0.5 ? "READY" : "GO!"}
          </PxText>
        )}
      </g>
      {dead && s.deadFor > 0.3 && (
        <g>
          <Px x={11} y={8} w={50} h={17} fill={c.accent} />
          <Px x={12} y={9} w={48} h={15} fill="#000" />
          <PxText x={W / 2} y={13.5 * U} size={13} fill={c.accent} shadow="#000" anchor="middle">
            {s.newBest ? "NEW BEST!" : "TUMBLED!"}
          </PxText>
          <PxText x={W / 2} y={16.8 * U} size={8} fill={c.text} shadow="#000" anchor="middle">
            {`SCORE ${pad4(s.score)}   BEST ${pad4(s.best)}   MAX x${s.maxMult}`}
          </PxText>
          <PxText x={W / 2} y={19.8 * U} size={7} fill={c.text} shadow="#000" anchor="middle">{s.deathLine}</PxText>
          <PxText x={W / 2} y={22.6 * U} size={7} fill={c.text} shadow="#000" anchor="middle">
            {Math.floor(s.deadFor * 3) % 2 === 0 ? "press an arrow to ride again" : ""}
          </PxText>
        </g>
      )}
    </GameFrame>
  );
}

/** The attract screen: the old hero's trail tableau — wagon, ox, mountains —
    doubling as the game's title card. */
function RunnerAttract(p: ThemePalette) {
  const c = sceneColors(p);
  const best = loadBest("dodge");
  const unlocked = loadUnlocked();
  const rider = loadRider(unlocked);
  return (
    <GameFrame label="Tumbleweed Dodge title screen: a covered wagon on the trail beneath mountains">
      <Prairie c={c} dist={0} />
      <WagonSprite x={39} y={15} canvas={c.snow} wood={c.mountain} dark={c.sky} />
      <RiderSprite rider={rider} c={c} x={31} y={24} step={0} />
      <WeedSprite x={51} y={25} spin={0.3} size="mid" fill={c.weed} dark={c.sky} />
      <CoinSprite x={21} y={21} t={0} fill={c.sun} bright={c.snow} />
      <CoinSprite x={24} y={17} t={0} fill={c.sun} bright={c.snow} />
      <TitlePlaque c={c} eyebrow="01 / PRAIRIE ARCADE · CHASE THE RUSH" title="TUMBLEWEED DODGE" subtitle={best > 0 ? `BEST ${pad4(best)} · COINS + CLOSE CALLS = STAMPEDE` : "COINS + CLOSE CALLS = STAMPEDE"} />
      {/* rider select: unlocked by distance, chosen with 1/2/3 */}
      <g fontFamily="var(--font-readout)" fontSize={8} textAnchor="middle">
        <Px x={0} y={ROWS - 5} w={COLS} h={5} fill={c.sky} />
        <Px x={0} y={ROWS - 5} w={COLS} h={0.25} fill={c.line} />
        {RIDERS.map((r, i) => {
          const open = unlocked.includes(r.id);
          const current = r.id === rider;
          const label = `${i + 1} ${current ? `[${r.label}]` : r.label}${open ? "" : ` (${r.yards} yd)`}`;
          const x = ((i + 0.5) * COLS * U) / RIDERS.length;
          return (
            <text key={r.id} x={x} y={(ROWS - 2) * U} fill={current ? c.accent : c.text} opacity={open ? 1 : 0.8}>
              {label}
            </text>
          );
        })}
      </g>
    </GameFrame>
  );
}

export const TumbleweedDodgeGame: HeroGameDefinition<RunnerState> = {
  title: "Tumbleweed Dodge",
  tab: "Dodge",
  seedSalt: 0x0d0d,
  initialState: () => resetRunner(loadBest("dodge")),
  onStart: (state) => resetRunner(Math.max(state.best, loadBest("dodge"))),
  handleKey: runnerKey,
  handleAttractKey: attractKey,
  handlePointer: runnerPointer,
  update: runnerUpdate,
  render: RunnerRender,
  renderAttract: RunnerAttract,
  help: "← → steer · ↑ / space hop · ↓ drop · coins + close calls charge Stampede",
};

// Exposed for tests.
export const _dodge = { resetRunner, laneX, OX_ROW, HZ, HOP_TIME, READY_TIME, MULT_DECAY, WAVE_LEN, BREATHER };
