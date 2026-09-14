// The Oxen Trail — a compact homage to the original 1978 Oregon Trail text game.
// You outfit a wagon at the general store, then play out the journey two weeks at
// a time: continue, hunt, and set your pace and rations while random events,
// river crossings, illness, and weather whittle down your party. Reach Oregon
// City before everyone dies. Rendered as a retro green-screen terminal, driven
// by the number keys (menus) and the arrow keys (the store and hunting).
//
// It plugs into the same HeroGameDefinition contract as the arcade game, so the
// hero can swap between the two cabinets. Because it's turn-based, `update` only
// advances a clock (for the blinking caret) — except while hunting, which embeds
// one real-time trip of the Hunting Season cabinet (see hunt.tsx). Every random
// draw happens inside `handleKey`, so a daily-seeded trail replays identically
// for the same key presses.

import type { ThemePalette } from "../../../lib/types";
import {
  clamp,
  COLS,
  GameFrame,
  loadBest,
  loadPref,
  pickRand,
  pointerAsKey,
  pushSfx,
  Px,
  poly,
  PxText,
  rand,
  randInt,
  ROWS,
  saveBest,
  savePref,
  sceneColors,
  U,
  type HeroGameDefinition,
  type PointerInput,
  type SfxEvent,
} from "./gameKit";
import type { SfxName } from "./sfx";
import { HuntScene, newTrip, terrainForMiles, tripKey, tripKeyUp, tripPause, tripResult, tripUpdate, type HuntTrip } from "./hunt";

type Phase = "occupation" | "store" | "trail" | "hunt" | "message" | "choice" | "river" | "over";
type Tone = "good" | "bad" | "neutral";

interface Member {
  name: string;
  alive: boolean;
  /** 0..100 — each traveler carries their own health; the party number is the average. */
  health: number;
  /** What ails them ("fever", "dysentery"…); cleared once they recover past 60. */
  ailment: string;
}

interface LegSummary {
  miles: number;
  ate: number;
  dh: number;
}

interface OregonState {
  phase: Phase;
  storeMode: "outfit" | "fort";
  clock: number; // seconds since play began — caret blink + hunt timer
  /** 0 banker · 1 carpenter · 2 farmer — less money, bigger score multiplier. */
  occupation: number;
  best: number;
  newBest: boolean;
  /** Where the finished run landed on the all-time table (0 = didn't place). */
  rank: number;
  /** A one-time cutoff offer, so the route choice can't be farmed. */
  cutoffOffered: boolean;
  /** Story flags set by decisions so later events can call back to them. */
  flags: Record<string, boolean>;
  sfx: SfxEvent[];

  // outfitting / trading
  budget: number; // dollars available in the current store screen
  spend: number[]; // dollars allocated per store row this screen
  cursor: number; // selected store row

  // party & inventory
  cash: number;
  miles: number;
  day: number;
  pace: number; // 0 steady · 1 strenuous · 2 grueling
  rations: number; // 0 filling · 1 meager · 2 bare bones
  oxen: number;
  food: number; // lbs
  bullets: number;
  clothing: number; // sets
  misc: number; // spare parts + medicine
  health: number; // derived: average health of the living, 0..100
  party: Member[];
  nextLandmark: number;
  atFort: boolean;
  weather: string;
  lastLeg: LegSummary | null;

  // message card
  msgTitle: string;
  msgLines: string[];
  msgTone: Tone;
  afterMessage: Phase;

  // a two-way decision card (see CHOICES): the option labels plus which
  // choice event is pending, resolved by key 1 or 2
  choiceId: string;
  choiceOptions: [string, string];

  // river crossing
  riverName: string;

  // hunting: one live trip of the hunting cabinet, or null off the field, and
  // how long to linger on the finished field before heading back on your own
  hunt: HuntTrip | null;
  huntLinger: number;
  huntSfxSeen: number;

  // outcome
  arrived: boolean;
  cause: string;
  epitaph: string;
  score: number;
}

const TRAIL_MILES = 2040;
const PARTY_NAMES = ["Wagon Boss", "Sarah", "Charlie", "Hank", "Milly"];
const STORE_ROWS = ["Oxen team", "Food", "Ammunition", "Clothing", "Supplies"];
const HUNT_SECONDS = 30;
const HUNT_CARRY = 100; // lbs you can haul back to the wagon, as in 1985
const HUNT_LINGER = 1.6; // seconds to admire the field before heading back
const CAUSES = ["dysentery", "typhoid fever", "cholera", "measles", "exhaustion", "a fever"];

const PACE_NAMES = ["Steady", "Strenuous", "Grueling"];
const RATION_NAMES = ["Filling", "Meager", "Bare bones"];
const RATION_LB = [55, 38, 24]; // lbs the party eats per leg
const PACE_MILES = [0, 24, 46];
const PACE_H = [4, -3, -10];
const RATION_H = [3, -3, -11];

// Occupation is the difficulty dial, as in the 1985 game: the banker rolls out
// rich, the farmer poor — but the farmer's score counts triple.
const OCCUPATIONS = [
  { name: "Banker", budget: 900, mult: 1, blurb: "from Boston · $900 · score x1" },
  { name: "Carpenter", budget: 700, mult: 2, blurb: "from Ohio · $700 · score x2" },
  { name: "Farmer", budget: 500, mult: 3, blurb: "from Illinois · $500 · score x3" },
];
const FERRY_COST = 20;

// Costs turning dollars into units when you leave a store screen.
const OX_COST = 40;
const FOOD_PER_$ = 5;
const AMMO_PER_$ = 10;
const CLOTH_COST = 10;
const MISC_COST = 5;

const LANDMARKS: { mile: number; name: string; type: "fort" | "river" | "flag" | "end" }[] = [
  { mile: 304, name: "Fort Kearney", type: "fort" },
  { mile: 554, name: "Chimney Rock", type: "flag" },
  { mile: 640, name: "Fort Laramie", type: "fort" },
  { mile: 830, name: "Independence Rock", type: "flag" },
  { mile: 932, name: "South Pass", type: "flag" },
  { mile: 1024, name: "Green River", type: "river" },
  { mile: 1288, name: "Fort Hall", type: "fort" },
  { mile: 1503, name: "Snake River", type: "river" },
  { mile: 1863, name: "The Dalles", type: "river" },
  { mile: TRAIL_MILES, name: "Oregon City", type: "end" },
];

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MDAYS = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

function dateStr(day: number) {
  let doy = 59 + day; // the journey sets out around March 1
  let year = 1848;
  while (doy >= 365) {
    doy -= 365;
    year++;
  }
  let m = 0;
  while (m < 11 && doy >= MDAYS[m]) {
    doy -= MDAYS[m];
    m++;
  }
  return `${MONTHS[m]} ${doy + 1}`;
}

const living = (s: OregonState) => s.party.filter((m) => m.alive);
const aliveCount = (s: OregonState) => living(s).length;
const healthWord = (h: number) => (h > 75 ? "Good" : h > 50 ? "Fair" : h > 25 ? "Poor" : "Very poor");
const shortName = (name: string) => (name === "Wagon Boss" ? "Boss" : name);

// ---- the all-time table and the last grave -----------------------------------
// Both live in localStorage so a first run has names to beat and the attract
// screen can show who you lost last time — the original's social hook.

interface ScoreEntry {
  name: string;
  score: number;
  occupation: string;
}

interface Tomb {
  name: string;
  cause: string;
  day: number;
}

const SEED_TABLE: ScoreEntry[] = [
  { name: "Marcus Whitman", score: 5400, occupation: "Banker" },
  { name: "Narcissa Prentiss", score: 4300, occupation: "Carpenter" },
  { name: "Jesse Applegate", score: 3600, occupation: "Farmer" },
  { name: "Tabitha Brown", score: 2900, occupation: "Carpenter" },
  { name: "Ezra Meeker", score: 2100, occupation: "Farmer" },
];

function loadTable(): ScoreEntry[] {
  try {
    const raw = loadPref("trail-scores");
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed) && parsed.length) return parsed;
    }
  } catch {
    // fall through to the seed
  }
  return SEED_TABLE;
}

/** Insert a finished run; returns its 1-based rank, or 0 if it didn't place. */
function recordScore(score: number, occupation: string): number {
  const table = [...loadTable(), { name: "You", score, occupation }].sort((a, b) => b.score - a.score).slice(0, 5);
  savePref("trail-scores", JSON.stringify(table));
  const i = table.findIndex((e) => e.name === "You" && e.score === score);
  return i < 0 ? 0 : i + 1;
}

function loadTomb(): Tomb | null {
  try {
    const raw = loadPref("trail-tomb");
    return raw ? (JSON.parse(raw) as Tomb) : null;
  } catch {
    return null;
  }
}

// ---- state helpers -----------------------------------------------------------

function freshGame(best = loadBest("trail")): OregonState {
  return {
    phase: "occupation",
    storeMode: "outfit",
    clock: 0,
    occupation: 0,
    best,
    newBest: false,
    rank: 0,
    cutoffOffered: false,
    flags: {},
    sfx: [],
    budget: 700,
    spend: [240, 180, 40, 60, 40],
    cursor: 0,
    cash: 0,
    miles: 0,
    day: 0,
    pace: 0,
    rations: 0,
    oxen: 0,
    food: 0,
    bullets: 0,
    clothing: 0,
    misc: 0,
    health: 100,
    party: PARTY_NAMES.map((name) => ({ name, alive: true, health: 100, ailment: "" })),
    nextLandmark: 0,
    atFort: false,
    weather: "Fair",
    lastLeg: null,
    msgTitle: "",
    msgLines: [],
    msgTone: "neutral",
    afterMessage: "trail",
    choiceId: "",
    choiceOptions: ["", ""],
    riverName: "",
    hunt: null,
    huntLinger: 0,
    huntSfxSeen: 0,
    arrived: false,
    cause: "",
    epitaph: "",
    score: 0,
  };
}

const cue = (s: OregonState, ...names: SfxName[]): OregonState => ({ ...s, sfx: pushSfx(s.sfx, ...names) });

/** Recompute the party average from the living. */
function refresh(s: OregonState): OregonState {
  const alive = s.party.filter((m) => m.alive);
  const health = alive.length ? Math.round(alive.reduce((sum, m) => sum + m.health, 0) / alive.length) : 0;
  return { ...s, health };
}

/** Apply `dh` to one traveler; healing past 60 shakes off whatever ailed them. */
function nudge(m: Member, dh: number, ailment?: string): Member {
  const health = clamp(m.health + dh, 0, 100);
  const recovered = dh > 0 && health > 60;
  return { ...m, health, ailment: recovered ? "" : (ailment ?? m.ailment) };
}

/** Everyone still standing takes `dh`. */
function hurtAll(s: OregonState, dh: number): OregonState {
  if (!dh) return refresh(s);
  return refresh({ ...s, party: s.party.map((m) => (m.alive ? nudge(m, dh) : m)) });
}

/** One traveler takes `dh` and (optionally) picks up an ailment. */
function hurtOne(s: OregonState, name: string, dh: number, ailment?: string): OregonState {
  return refresh({ ...s, party: s.party.map((m) => (m.alive && m.name === name ? nudge(m, dh, ailment) : m)) });
}

const randomMember = (s: OregonState): Member => pickRand(living(s));

function toMessage(s: OregonState, title: string, lines: string[], tone: Tone, after: Phase): OregonState {
  const next = { ...s, phase: "message" as Phase, msgTitle: title, msgLines: lines, msgTone: tone, afterMessage: after };
  return tone === "neutral" ? next : cue(next, tone);
}

function scoreOf(s: OregonState): number {
  const raw =
    aliveCount(s) * 350 +
    Math.floor(s.cash) +
    Math.floor(s.food / 8) +
    s.oxen * 4 +
    s.misc * 3 +
    s.clothing * 3 +
    Math.round(s.health * 2);
  return raw * OCCUPATIONS[s.occupation].mult;
}

/** Terminal bookkeeping shared by arrival and the tombstone. */
function finish(s: OregonState): OregonState {
  const score = scoreOf(s);
  const best = saveBest("trail", score);
  const newBest = s.best > 0 ? score > s.best : false;
  const rank = recordScore(score, OCCUPATIONS[s.occupation].name);
  return cue({ ...s, score, best, newBest, rank }, newBest ? "best" : "good");
}

// ---- random events ---------------------------------------------------------
// Each event returns the message to show and the fields it changes. Absolute
// values keep them easy to read. Health goes through `dhAll` (everyone) or
// `victim` (one traveler, who may pick up an ailment) so the party's individual
// health lines stay honest.

interface EventResult {
  title: string;
  lines: string[];
  tone: Tone;
  changes: Partial<OregonState>;
  dhAll?: number;
  victim?: { name: string; dh: number; ailment?: string };
}

interface TrailEvent {
  when?: (s: OregonState) => boolean;
  weight: number;
  run: (s: OregonState) => EventResult;
}

const EVENTS: TrailEvent[] = [
  {
    weight: 3,
    run: (s) => ({
      title: "Wagon breaks down",
      lines: s.misc > 0 ? ["A wheel cracks. You have the", "parts to mend it, losing a day."] : ["A wheel cracks and you have no", "spare parts. Repairs cost days."],
      tone: "bad",
      changes: { misc: Math.max(0, s.misc - 1), day: s.day + (s.misc > 0 ? 1 : 3) },
      dhAll: -(s.misc > 0 ? 2 : 8),
    }),
  },
  {
    weight: 2,
    run: (s) => ({
      title: "An ox wanders off",
      lines: ["You spend half a day", "rounding it up again."],
      tone: "bad",
      changes: { day: s.day + 1 },
    }),
  },
  {
    weight: 2,
    run: (s) => {
      const spent = randInt(15, 35);
      return {
        title: "Wild animals attack",
        lines: [`You drive them off, spending`, `${spent} bullets in the night.`],
        tone: "bad",
        changes: { bullets: Math.max(0, s.bullets - spent) },
        dhAll: s.bullets < spent ? -10 : 0,
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      // A fed dog earns its keep: it wakes the camp and the raid is a fumble.
      const dog = !!s.flags.dog;
      const stolen = Math.round(randInt(60, 160) / (dog ? 2 : 1));
      const bullets = Math.round(randInt(10, 30) / (dog ? 2 : 1));
      return {
        title: "Bandits attack!",
        lines: dog ? ["The dog raised the alarm. They", `still grab ${stolen} lbs of food.`] : [`They make off with about`, `${stolen} lbs of your food.`],
        tone: "bad",
        changes: { food: Math.max(0, s.food - stolen), bullets: Math.max(0, s.bullets - bullets) },
      };
    },
  },
  {
    weight: 2,
    run: (s) => ({
      title: "Unsafe water",
      lines: ["Bad water sickens the party.", "You lose time finding a spring."],
      tone: "bad",
      changes: { day: s.day + 1 },
      dhAll: -10,
    }),
  },
  {
    weight: 2,
    run: (s) => ({
      title: "Heavy rains",
      lines: ["The trail turns to mud and", "some of your food spoils."],
      tone: "bad",
      changes: { day: s.day + 1, food: Math.max(0, s.food - randInt(20, 45)) },
    }),
  },
  {
    weight: 2,
    run: (s) => ({
      title: "Hail storm",
      lines: ["Hail hammers the wagon and", "damages your supplies."],
      tone: "bad",
      changes: { misc: Math.max(0, s.misc - 1), food: Math.max(0, s.food - randInt(10, 30)) },
    }),
  },
  {
    weight: 1,
    run: (s) => ({
      title: "Fire in the wagon!",
      lines: ["You lose food and supplies", "to the flames."],
      tone: "bad",
      changes: { food: Math.max(0, s.food - randInt(30, 70)), misc: Math.max(0, s.misc - 1) },
    }),
  },
  {
    weight: 2,
    run: (s) => {
      const victim = randomMember(s);
      const treated = s.misc > 0;
      return {
        title: "Snakebite!",
        lines: treated ? [`A rattlesnake bites ${victim.name}.`, "Your medicine pulls them through."] : [`A rattlesnake bites ${victim.name}`, "and you have no medicine."],
        tone: "bad",
        changes: { misc: Math.max(0, s.misc - 1) },
        victim: { name: victim.name, dh: -(treated ? 12 : 40), ailment: "snakebite" },
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      const victim = randomMember(s);
      const treated = s.misc > 0;
      return {
        title: "Dysentery strikes",
        lines: treated ? [`${victim.name} falls ill, but your`, "medicine holds it off."] : [`${victim.name} falls ill and you`, "have no medicine to treat it."],
        tone: "bad",
        changes: { misc: Math.max(0, s.misc - 1) },
        dhAll: -(treated ? 2 : 6),
        victim: { name: victim.name, dh: -(treated ? 14 : 40), ailment: "dysentery" },
      };
    },
  },
  {
    when: (s) => s.miles > 950,
    weight: 3,
    run: (s) => {
      const clothed = s.clothing >= aliveCount(s);
      return {
        title: "Cold weather — brrr!",
        lines: clothed ? ["Bitter cold in the high country,", "but you have clothing enough."] : ["Bitter cold and not enough", "clothing to go around."],
        tone: clothed ? "neutral" : "bad",
        changes: { weather: "Cold" },
        dhAll: -(clothed ? 3 : 15),
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      const found = randInt(30, 70);
      return {
        title: "Helpful travelers",
        lines: [`Friendly travelers share the`, `way to ${found} lbs of food.`],
        tone: "good",
        changes: { food: s.food + found },
      };
    },
  },
  {
    weight: 1,
    run: () => ({
      title: "Good trail",
      lines: ["The trail is dry and clear.", "Spirits lift as you roll on."],
      tone: "good",
      changes: {},
      dhAll: 6,
    }),
  },
];

// ---- decisions -------------------------------------------------------------
// Most bad luck should trace back to something you chose. These events pose a
// two-way call; `resolve` runs when you press 1 or 2 on the card. Some set
// flags so a later card can call back to what you did.

interface ChoiceEvent {
  id: string;
  when?: (s: OregonState) => boolean;
  weight: number;
  title: string;
  lines: (s: OregonState) => string[];
  options: [string, string];
  resolve: (s: OregonState, pick: 1 | 2) => EventResult;
}

const sickest = (s: OregonState): Member => living(s).reduce((a, b) => (b.health < a.health ? b : a));

const CHOICES: ChoiceEvent[] = [
  {
    id: "wagon",
    weight: 3,
    title: "An abandoned wagon",
    lines: () => ["A wagon sits off the trail,", "its owners nowhere in sight."],
    options: ["Search it", "Pass it by"],
    resolve: (s, pick) => {
      if (pick === 2) return { title: "You pass it by", lines: ["Better safe than sorry.", "The trail rolls on."], tone: "neutral", changes: {} };
      if (rand() < 0.6) {
        const food = randInt(40, 90);
        return { title: "A lucky find", lines: [`You find ${food} lbs of food`, "and a spare wagon part."], tone: "good", changes: { food: s.food + food, misc: s.misc + 1 } };
      }
      const victim = randomMember(s);
      return { title: "Cholera!", lines: ["The wagon was abandoned for", `a reason. ${victim.name} falls ill.`], tone: "bad", changes: {}, dhAll: -6, victim: { name: victim.name, dh: -24, ailment: "cholera" } };
    },
  },
  {
    id: "trader",
    weight: 3,
    when: (s) => s.bullets >= 40 && !s.flags.refusedTrader,
    title: "A trader hails you",
    lines: () => ["He offers 100 lbs of food", "for 40 of your bullets."],
    options: ["Trade", "Decline"],
    resolve: (s, pick) =>
      pick === 1
        ? { title: "Deal done", lines: ["You hand over the bullets and", "load up 100 lbs of food."], tone: "good", changes: { food: s.food + 100, bullets: s.bullets - 40 } }
        : { title: "No deal", lines: ["He shrugs and rides off", "toward the sunset."], tone: "neutral", changes: { flags: { ...s.flags, refusedTrader: true } } },
  },
  {
    id: "traderBack",
    weight: 3,
    when: (s) => !!s.flags.refusedTrader && !s.flags.traderBack && s.bullets >= 60,
    title: "The trader returns",
    lines: () => ["Same trader, worse mood: 100 lbs", "of food for 60 bullets now."],
    options: ["Take it", "Refuse again"],
    resolve: (s, pick) =>
      pick === 1
        ? { title: "A dearer deal", lines: ["You pay his price. The food", "goes in the wagon."], tone: "good", changes: { food: s.food + 100, bullets: s.bullets - 60, flags: { ...s.flags, traderBack: true } } }
        : { title: "He spits and rides", lines: ["You won't see him again.", "Good riddance."], tone: "neutral", changes: { flags: { ...s.flags, traderBack: true } } },
  },
  {
    id: "fever",
    weight: 3,
    title: "Fever in the wagon",
    lines: (s) => [`${sickest(s).name} is burning up.`, "Do you stop to rest?"],
    options: ["Rest 3 days", "Push on"],
    resolve: (s, pick) => {
      const sick = sickest(s);
      if (pick === 1)
        return { title: "You make camp", lines: [`Three days of rest and ${sick.name}'s`, "fever breaks. Spirits lift."], tone: "good", changes: { day: s.day + 3, food: Math.max(0, s.food - 20) }, dhAll: 5, victim: { name: sick.name, dh: 20 } };
      return rand() < 0.5
        ? { title: "The fever passes", lines: ["Rough days on the road, but", `${sick.name} pulls through.`], tone: "neutral", changes: {}, victim: { name: sick.name, dh: -4, ailment: "fever" } }
        : { title: "It gets worse", lines: ["Jolting along made it worse.", `${sick.name} is fading.`], tone: "bad", changes: {}, victim: { name: sick.name, dh: -25, ailment: "fever" } };
    },
  },
  {
    id: "beggars",
    weight: 2,
    when: (s) => s.food > 120,
    title: "Hungry travelers",
    lines: () => ["A family on foot begs for", "30 lbs of food."],
    options: ["Share it", "Keep it"],
    resolve: (s, pick) =>
      pick === 1
        ? { title: "They thank you", lines: ["They point out a spring ahead", "and a shortcut around the bluff."], tone: "good", changes: { food: s.food - 30, miles: s.miles + 12 }, dhAll: 6 }
        : { title: "You move on", lines: ["You can't feed the whole", "trail. Nobody says a word."], tone: "neutral", changes: {}, dhAll: -2 },
  },
  {
    id: "cutoff",
    weight: 4,
    when: (s) => !s.cutoffOffered && s.miles > 900 && s.miles < 1400,
    title: "A cutoff!",
    lines: () => ["A rough track promises to", "shave 100 miles off the trail."],
    options: ["Take it", "Stay on the trail"],
    resolve: (s, pick) => {
      if (pick === 2) return { title: "You stay the course", lines: ["Slow and steady. The known", "trail it is."], tone: "neutral", changes: { cutoffOffered: true } };
      if (rand() < 0.6) return { title: "The cutoff pays off", lines: ["Rough going, but you save", "100 miles of trail."], tone: "good", changes: { cutoffOffered: true, miles: s.miles + 100 }, dhAll: -4 };
      return { title: "Lost in the badlands", lines: ["The track peters out. You", "lose four days finding the trail."], tone: "bad", changes: { cutoffOffered: true, day: s.day + 4, food: Math.max(0, s.food - 40) }, dhAll: -12 };
    },
  },
  {
    id: "thief",
    weight: 2,
    when: (s) => s.food > 60,
    title: "A thief in the night",
    lines: () => ["Someone's rifling the wagon.", "He bolts with a sack of food."],
    options: ["Chase him", "Let him go"],
    resolve: (s, pick) => {
      if (pick === 2) return { title: "He gets away", lines: ["Forty pounds of food, gone", "into the dark."], tone: "bad", changes: { food: Math.max(0, s.food - 40) } };
      if (rand() < 0.5) return { title: "You run him down", lines: ["You get the food back and", "20 bullets from his pack."], tone: "good", changes: { bullets: s.bullets + 20 } };
      return { title: "Lost in the dark", lines: ["You lose him, a day, and", "a good night's sleep."], tone: "bad", changes: { day: s.day + 1, food: Math.max(0, s.food - 40) }, dhAll: -6 };
    },
  },
  {
    id: "lameOx",
    weight: 2,
    when: (s) => s.oxen >= 1,
    title: "A lame ox",
    lines: () => ["One of the oxen is limping", "badly. It can't pull today."],
    options: ["Rest 2 days", "Put it down"],
    resolve: (s, pick) =>
      pick === 1
        ? { title: "The ox recovers", lines: ["Two days of rest and it", "steps out sound again."], tone: "neutral", changes: { day: s.day + 2, food: Math.max(0, s.food - 15) } }
        : { title: "Fresh meat", lines: ["A hard day. The team is one", "ox short, the larder 150 lbs fuller."], tone: "neutral", changes: { oxen: Math.max(0, s.oxen - 1), food: s.food + 150 } },
  },
  {
    id: "fruit",
    weight: 2,
    title: "Wild fruit by the trail",
    lines: () => ["Bushes heavy with berries", "line the creek."],
    options: ["Gather them", "Pass"],
    resolve: (s, pick) => {
      if (pick === 2) return { title: "You pass", lines: ["No time for berry picking.", "Oregon won't wait."], tone: "neutral", changes: {} };
      if (rand() < 0.8) return { title: "A sweet haul", lines: ["Thirty pounds of berries and", "the whole wagon in better spirits."], tone: "good", changes: { food: s.food + 30 }, dhAll: 4 };
      const victim = randomMember(s);
      return { title: "Bad berries", lines: [`${victim.name} ate the wrong ones`, "and is sick for days."], tone: "bad", changes: {}, victim: { name: victim.name, dh: -12, ailment: "bad berries" } };
    },
  },
  {
    id: "dog",
    weight: 2,
    when: (s) => !s.flags.dog && !s.flags.shooedDog,
    title: "A stray dog",
    lines: () => ["A thin dog has followed the", "wagon since noon, hopeful."],
    options: ["Feed it", "Shoo it off"],
    resolve: (s, pick) =>
      pick === 1
        ? { title: "You have a dog", lines: ["It eats 15 lbs and takes up", "a post by the wagon at night."], tone: "good", changes: { food: Math.max(0, s.food - 15), flags: { ...s.flags, dog: true } }, dhAll: 2 }
        : { title: "It slinks away", lines: ["Milly won't speak to you", "for a day."], tone: "neutral", changes: { flags: { ...s.flags, shooedDog: true } } },
  },
  {
    id: "gambler",
    weight: 3,
    when: (s) => s.atFort && s.cash >= 40,
    title: "A gambler at the fort",
    lines: () => ["A card sharp offers to double", "$40 on a single cut."],
    options: ["Wager $40", "Decline"],
    resolve: (s, pick) => {
      if (pick === 2) return { title: "You keep your money", lines: ["He finds another mark", "before you've turned around."], tone: "neutral", changes: {} };
      return rand() < 0.5
        ? { title: "You win!", lines: ["High card. He pays up with", "a sour look. +$40."], tone: "good", changes: { cash: s.cash + 40 } }
        : { title: "You lose", lines: ["Low card. $40 lighter and", "a lesson learned."], tone: "bad", changes: { cash: s.cash - 40 } };
    },
  },
  {
    id: "storm",
    weight: 2,
    title: "Storm on the horizon",
    lines: () => ["Black clouds stack up in the", "west. The wind is rising."],
    options: ["Make camp", "Push through"],
    resolve: (s, pick) => {
      if (pick === 1) return { title: "You wait it out", lines: ["Two days under canvas, but", "nobody gets hurt."], tone: "neutral", changes: { day: s.day + 2, food: Math.max(0, s.food - 15) } };
      if (rand() < 0.6) return { title: "You outrun it", lines: ["The storm passes behind you.", "A day gained."], tone: "good", changes: {} };
      return { title: "Caught in the open", lines: ["Hail and lightning. Supplies", "lost and everyone soaked."], tone: "bad", changes: { misc: Math.max(0, s.misc - 1) }, dhAll: -10 };
    },
  },
];

function rollChoice(s: OregonState): ChoiceEvent | null {
  const pool = CHOICES.filter((e) => !e.when || e.when(s));
  const total = pool.reduce((sum, e) => sum + e.weight, 0);
  let r = rand() * total;
  for (const e of pool) {
    r -= e.weight;
    if (r <= 0) return e;
  }
  return null;
}

function toChoice(s: OregonState, e: ChoiceEvent): OregonState {
  return cue({ ...s, phase: "choice", msgTitle: e.title, msgLines: e.lines(s), choiceId: e.id, choiceOptions: e.options }, "menu");
}

function applyResult(s: OregonState, r: EventResult): OregonState {
  let next = { ...s, ...r.changes } as OregonState;
  if (r.dhAll) next = hurtAll(next, r.dhAll);
  if (r.victim) next = hurtOne(next, r.victim.name, r.victim.dh, r.victim.ailment);
  return refresh(next);
}

/** The first traveler whose health has run out, if any. */
const fallen = (s: OregonState): Member | undefined => living(s).find((m) => m.health <= 0);

function resolveChoice(s: OregonState, pickNo: 1 | 2): OregonState {
  const e = CHOICES.find((c) => c.id === s.choiceId);
  if (!e) return { ...s, phase: "trail" };
  const r = e.resolve(s, pickNo);
  let next = applyResult(s, r);
  // A cutoff can carry you across a landmark; keep the landmark index honest.
  while (LANDMARKS[next.nextLandmark] && LANDMARKS[next.nextLandmark].type !== "end" && next.miles >= LANDMARKS[next.nextLandmark].mile) {
    next = { ...next, nextLandmark: next.nextLandmark + 1 };
  }
  if (next.miles >= TRAIL_MILES) return arrive(next);
  const dead = fallen(next);
  if (dead) return afflict(next, dead);
  return toMessage(next, r.title, r.lines, r.tone, "trail");
}

function rollEvent(s: OregonState): EventResult | null {
  const pool = EVENTS.filter((e) => !e.when || e.when(s));
  const total = pool.reduce((sum, e) => sum + e.weight, 0);
  let r = rand() * total;
  for (const e of pool) {
    r -= e.weight;
    if (r <= 0) return e.run(s);
  }
  return null;
}

// ---- outcomes --------------------------------------------------------------

function arrive(s: OregonState): OregonState {
  const next = finish({ ...s, miles: TRAIL_MILES, arrived: true });
  return toMessage(next, "OREGON CITY!", [`You reached Oregon in ${next.day} days`, `with ${aliveCount(next)} of 5 still standing.`, `Final score: ${next.score}`], "good", "over");
}

// A traveler's health has run out: they are lost, of whatever ailed them. The
// others get a floor so one bad leg doesn't wipe the wagon; when the last one
// goes, the run is over (the classic tombstone).
function afflict(s: OregonState, victim: Member): OregonState {
  const cause = victim.ailment || pickRand(CAUSES);
  const party = s.party.map((m) => (m.name === victim.name ? { ...m, alive: false, health: 0 } : m.alive ? { ...m, health: Math.max(30, m.health) } : m));
  let next = refresh({ ...s, party });
  savePref("trail-tomb", JSON.stringify({ name: victim.name, cause, day: s.day } satisfies Tomb));
  if (party.some((m) => m.alive)) {
    next = { ...next, misc: Math.max(0, s.misc - 1) };
    return toMessage(next, `${victim.name} has died`, [`of ${cause}.`, "The rest of the party carries on."], "bad", "trail");
  }
  next = { ...next, arrived: false, cause, epitaph: victim.name };
  return cue({ ...finish(next), phase: "over" }, "bad");
}

// One two-week leg of travel: eat, advance, weather, health, then whatever the
// trail throws at you (arrival, a landmark, an event, or a quiet stretch).
function travel(s: OregonState): OregonState {
  const alive = aliveCount(s);
  const eat = alive * RATION_LB[s.rations];
  const starving = s.food < eat;
  const ate = Math.min(s.food, eat);
  const food = Math.max(0, s.food - eat);

  const landmark = LANDMARKS[s.nextLandmark];
  let base = 110 + s.oxen * 8 + PACE_MILES[s.pace] + randInt(0, 26);
  if (s.health < 45) base -= 25;
  base = Math.max(35, base);

  let miles = s.miles;
  let reached = false;
  if (landmark && miles + base >= landmark.mile) {
    miles = landmark.mile;
    reached = true;
  } else {
    miles += base;
  }

  const cold = miles > 950 && rand() < 0.5;
  const weather = miles > 1500 ? (cold ? "Snow" : "Cold") : cold ? "Cold" : pickRand(["Fair", "Clear", "Rainy", "Windy"]);

  let dh = PACE_H[s.pace] + RATION_H[s.rations];
  if (starving) dh -= 22;
  if (cold && s.clothing < alive) dh -= 14;

  let next: OregonState = hurtAll(
    {
      ...s,
      food,
      miles,
      day: s.day + 14,
      weather,
      atFort: false,
      lastLeg: { miles: miles - s.miles, ate, dh },
    },
    dh,
  );

  // A quiet leg can still spring an event (never on the same leg you reach a
  // landmark — that gets the spotlight). Most legs pose a decision; pure luck
  // stays in the mix for tension.
  let eventMsg: EventResult | null = null;
  let choice: ChoiceEvent | null = null;
  if (!reached) {
    const roll = rand();
    if (roll < 0.32) choice = rollChoice(next);
    else if (roll < 0.62) eventMsg = rollEvent(next);
    if (eventMsg) next = applyResult(next, eventMsg);
  }

  if (next.miles >= TRAIL_MILES) return arrive(next);
  const dead = fallen(next);
  if (dead) return afflict(next, dead);
  if (choice) return toChoice(next, choice);

  if (reached) {
    next = cue({ ...next, nextLandmark: s.nextLandmark + 1 }, "milestone");
    if (landmark.type === "end") return arrive(next);
    if (landmark.type === "fort")
      return toMessage({ ...next, atFort: true }, `You reach ${landmark.name}`, ["Rest here and trade for", "fresh supplies before pressing on."], "good", "trail");
    if (landmark.type === "river") return { ...next, phase: "river", riverName: landmark.name };
    return toMessage(next, `You reach ${landmark.name}`, ["A welcome landmark — Oregon", "is another step closer."], "good", "trail");
  }

  if (eventMsg) return toMessage(next, eventMsg.title, eventMsg.lines, eventMsg.tone, "trail");
  return { ...next, phase: "trail" };
}

function crossRiver(s: OregonState, choice: number): OregonState {
  const river = s.riverName;
  const next = { ...s, nextLandmark: s.nextLandmark + 1, phase: "trail" as Phase, riverName: "" };
  const deep = rand();

  if (choice === 3) {
    // Wait for better conditions, then cross safely.
    return toMessage({ ...next, day: s.day + randInt(1, 3) }, `You wait at the ${river}`, ["The waters calm after a few days", "and you cross without trouble."], "neutral", "trail");
  }
  if (choice === 4) {
    if (s.cash < FERRY_COST) return s;
    return toMessage({ ...next, cash: s.cash - FERRY_COST, day: s.day + 1 }, `You take the ferry`, [`$${FERRY_COST} lighter, the ${river}`, "is behind you without a splash."], "good", "trail");
  }

  const ford = choice === 1;
  const disaster = ford ? deep > 0.55 : deep > 0.78; // fording is riskier

  if (!disaster) {
    return toMessage(next, `You cross the ${river}`, ford ? ["You ford the river and reach", "the far bank safely."] : ["You caulk the wagon and float", "across without a hitch."], "good", "trail");
  }

  // Something goes wrong in the water.
  if (rand() < 0.4) {
    const victim = randomMember(s);
    return afflict(hurtOne(next, victim.name, -100, "drowning"), { ...victim, health: 0, ailment: "drowning" });
  }
  const lostFood = randInt(80, 200);
  const lostOx = ford && s.oxen > 1 && rand() < 0.5 ? 1 : 0;
  return toMessage(
    hurtAll({ ...next, food: Math.max(0, s.food - lostFood), oxen: s.oxen - lostOx }, -12),
    `The ${river} nearly takes you`,
    [`The wagon tips and you lose`, `${lostFood} lbs of food${lostOx ? " and an ox" : ""}.`],
    "bad",
    "trail",
  );
}

// ---- hunting -----------------------------------------------------------------
// A hunt is one real-time trip on the field (hunt.tsx): your actual bullets,
// the terrain you're actually in, thirty seconds, and the 100 lb carry limit.
// It costs a day on the trail, so it's a time-versus-food call, not free food.

function startHunt(s: OregonState): OregonState {
  if (s.bullets < 1) return toMessage(s, "Out of bullets", ["You need more ammunition", "before you can hunt."], "bad", "trail");
  return {
    ...s,
    phase: "hunt",
    huntLinger: HUNT_LINGER,
    huntSfxSeen: 0,
    hunt: newTrip({ terrain: terrainForMiles(s.miles), bullets: s.bullets, seconds: HUNT_SECONDS, carryCap: HUNT_CARRY, warmup: 0.6 }),
  };
}

/** Relay any cues the trip queued (if the hunt module keeps an `sfx` list). */
function withHunt(s: OregonState, hunt: HuntTrip): OregonState {
  const queue = (hunt as unknown as { sfx?: SfxEvent[] }).sfx;
  if (!queue || queue.length === 0) return { ...s, hunt };
  const fresh = queue.filter((e) => e.id > s.huntSfxSeen).map((e) => e.name);
  const seen = queue[queue.length - 1].id;
  return fresh.length ? { ...s, hunt, huntSfxSeen: seen, sfx: pushSfx(s.sfx, ...fresh) } : { ...s, hunt, huntSfxSeen: seen };
}

function resolveHunt(s: OregonState): OregonState {
  const trip = s.hunt;
  if (!trip) return { ...s, phase: "trail" };
  const r = tripResult(trip);
  const next = { ...s, hunt: null, day: s.day + 1, bullets: trip.bulletsLeft, food: s.food + r.carried };
  if (r.mauled) {
    const victim = randomMember(s);
    const hurt = hurtOne(next, victim.name, -20, "mauling");
    const dead = fallen(hurt);
    if (dead) return afflict(hurt, dead);
    return toMessage(hurt, "A bear got the better of you", [`It mauled ${victim.name} before you`, `got clear${r.carried ? ` with ${r.carried} lbs of meat` : ""}.`], "bad", "trail");
  }
  if (r.shot === 0) return toMessage(next, "Nothing to show for it", [`${r.bulletsUsed} bullets spent and the`, "game got away. A day lost."], "bad", "trail");
  if (r.wasted > 0)
    return toMessage(next, `You shot ${r.shot} lbs of meat`, [`but could only carry ${r.carried} lbs`, `back to the wagon. ${r.wasted} lbs wasted.`], "neutral", "trail");
  return toMessage(next, "Good hunting!", [`You bring back ${r.carried} lbs of`, `fresh meat with ${r.bulletsUsed} bullets.`], "good", "trail");
}

// ---- input -----------------------------------------------------------------

function storeKey(s: OregonState, key: string): OregonState {
  if (key === "ArrowUp") return { ...s, cursor: (s.cursor + STORE_ROWS.length - 1) % STORE_ROWS.length };
  if (key === "ArrowDown") return { ...s, cursor: (s.cursor + 1) % STORE_ROWS.length };

  const spent = s.spend.reduce((a, b) => a + b, 0);
  if (key === "ArrowLeft") {
    if (s.spend[s.cursor] === 0) return s;
    const spend = s.spend.slice();
    spend[s.cursor] = Math.max(0, spend[s.cursor] - 10);
    return { ...s, spend };
  }
  if (key === "ArrowRight") {
    if (spent + 10 > s.budget) return s;
    const spend = s.spend.slice();
    spend[s.cursor] += 10;
    return { ...s, spend };
  }
  if (key === "Enter") {
    const [oxSpend, foodSpend, ammoSpend, clothSpend, miscSpend] = s.spend;
    const bought = {
      oxen: Math.round(oxSpend / OX_COST),
      food: foodSpend * FOOD_PER_$,
      bullets: ammoSpend * AMMO_PER_$,
      clothing: Math.round(clothSpend / CLOTH_COST),
      misc: Math.round(miscSpend / MISC_COST),
    };
    const leftover = s.budget - spent;
    if (s.storeMode === "outfit") {
      if (bought.oxen < 1) return toMessage(s, "Hold on there", ["You need at least one team of", "oxen to pull the wagon."], "bad", "store");
      if (bought.food < 200) return toMessage(s, "Not enough food", ["Lay in more food or the party", "will starve on the trail."], "bad", "store");
      return { ...s, phase: "trail", cash: leftover, ...bought };
    }
    // Fort restock: fold the purchase into what you already carry.
    return {
      ...s,
      phase: "trail",
      atFort: false,
      cash: leftover,
      oxen: s.oxen + bought.oxen,
      food: s.food + bought.food,
      bullets: s.bullets + bought.bullets,
      clothing: s.clothing + bought.clothing,
      misc: s.misc + bought.misc,
    };
  }
  return s;
}

function trailKey(s: OregonState, key: string): OregonState {
  if (key === "1") return travel(s);
  if (key === "2") return startHunt(s);
  if (key === "3") return { ...s, pace: (s.pace + 1) % PACE_NAMES.length };
  if (key === "4") return { ...s, rations: (s.rations + 1) % RATION_NAMES.length };
  if (key === "5" && s.atFort)
    return { ...s, phase: "store", storeMode: "fort", budget: s.cash, spend: [0, 0, 0, 0, 0], cursor: 1 };
  return s;
}

function huntKey(s: OregonState, key: string): OregonState {
  if (!s.hunt) return { ...s, phase: "trail" };
  if (s.hunt.done) return resolveHunt(s);
  return withHunt(s, tripKey(s.hunt, key));
}

function oregonKeyUp(s: OregonState, key: string): OregonState {
  if (s.phase !== "hunt" || !s.hunt) return s;
  return withHunt(s, tripKeyUp(s.hunt, key));
}

function occupationKey(s: OregonState, key: string): OregonState {
  const i = ["1", "2", "3"].indexOf(key);
  if (i < 0) return s;
  const occ = OCCUPATIONS[i];
  // Seed the store split proportionally so every wallet starts sensible.
  const f = occ.budget / 700;
  const spend = [240, 180, 40, 60, 40].map((d) => Math.round((d * f) / 10) * 10);
  return { ...s, phase: "store", occupation: i, budget: occ.budget, spend };
}

function oregonKeyInner(s: OregonState, key: string): OregonState {
  switch (s.phase) {
    case "occupation":
      return occupationKey(s, key);
    case "choice":
      if (key === "1" || key === "2") return resolveChoice(s, key === "1" ? 1 : 2);
      return s;
    case "store":
      return storeKey(s, key);
    case "trail":
      return trailKey(s, key);
    case "hunt":
      return huntKey(s, key);
    case "message":
      return { ...s, phase: s.afterMessage };
    case "river":
      if (key === "1" || key === "2" || key === "3" || key === "4") return crossRiver(s, Number(key));
      return s;
    case "over":
      return freshGame(s.best);
    default:
      return s;
  }
}

/** Every accepted menu press clicks, unless the outcome already brought its own cue. */
function oregonKey(s: OregonState, key: string): OregonState {
  const next = oregonKeyInner(s, key);
  if (next === s || s.phase === "hunt" || next.sfx !== s.sfx) return next;
  return cue(next, "menu");
}

// Pointer play: taps land on whatever the screen shows at that height, so the
// numbered menus work by touch without a keyboard. Layout constants mirror the
// render functions below.
const MENU_TOP = 88;
const MENU_STEP = 10;
const RIVER_TOP = 86;
const RIVER_STEP = 11;

function rowAt(yFrac: number, top: number, step: number, count: number) {
  return clamp(Math.round((yFrac * H - top) / step), 0, count - 1);
}

function oregonPointer(s: OregonState, p: PointerInput): OregonState {
  if (p.kind === "swipe") return oregonKey(s, pointerAsKey(p));
  switch (s.phase) {
    case "message":
    case "over":
      return oregonKey(s, "Enter");
    case "choice":
      return oregonKey(s, p.x < 0.5 ? "1" : "2");
    case "river":
      return oregonKey(s, String(rowAt(p.y, RIVER_TOP, RIVER_STEP, 4) + 1));
    case "trail":
      return oregonKey(s, String(rowAt(p.y, MENU_TOP, MENU_STEP, s.atFort ? 5 : 4) + 1));
    case "store":
      if (p.y < 1 / 3) return oregonKey(s, "ArrowUp");
      if (p.y > 2 / 3) return oregonKey(s, "ArrowDown");
      if (p.x < 0.35) return oregonKey(s, "ArrowLeft");
      if (p.x > 0.65) return oregonKey(s, "ArrowRight");
      return oregonKey(s, "Enter");
    case "occupation":
      return oregonKey(s, String(Math.min(3, Math.floor(p.y * 3) + 1)));
    case "hunt":
      return oregonKey(s, " ");
    default:
      return s;
  }
}

function oregonUpdate(s: OregonState, dt: number): OregonState {
  const clock = s.clock + dt;
  if (s.phase === "hunt" && s.hunt) {
    const hunt = tripUpdate(s.hunt, dt);
    if (!hunt.done) return withHunt({ ...s, clock }, hunt);
    // Linger a beat on the finished field so the last drop can be seen, then
    // any key (or the clock) hands the bag back to the wagon.
    const huntLinger = s.huntLinger - dt;
    if (huntLinger <= 0) return resolveHunt(withHunt({ ...s, clock }, hunt));
    return withHunt({ ...s, clock, huntLinger }, hunt);
  }
  return { ...s, clock };
}

// ---- rendering -------------------------------------------------------------

function Line({ x, y, c, size = 8, anchor, children }: { x: number; y: number; c: string; size?: number; anchor?: "start" | "middle" | "end"; children: string }) {
  return (
    <text x={x} y={y} fill={c} fontFamily="var(--font-readout)" fontSize={size} textAnchor={anchor} xmlSpace="preserve">
      {children}
    </text>
  );
}

const W = COLS * U; // screen width in viewBox units
const H = ROWS * U;

function screenColors(p: ThemePalette) {
  const c = sceneColors(p);
  return {
    bg: c.sky,
    fg: c.text,
    dim: c.mountainBack || c.line,
    accent: c.accent,
    good: c.grass,
    bad: c.danger,
    frame: c.line,
  };
}

type ScreenColors = ReturnType<typeof screenColors>;

const signed = (n: number) => (n > 0 ? `+${n}` : `${n}`);

/** The status header: a progress bar marked with every landmark ahead and
    behind, and a wagon inching toward Oregon. */
function Header(s: OregonState, sc: ScreenColors) {
  const frac = clamp(s.miles / TRAIL_MILES, 0, 1);
  const barX = 10;
  const barW = W - 20;
  const wagonX = barX + barW * frac;
  const barY = 20;
  return (
    <g>
      <Line x={10} y={13} c={sc.fg} size={9}>{`Day ${s.day}`}</Line>
      <Line x={W / 2} y={13} c={sc.dim} size={9} anchor="middle">{dateStr(s.day)}</Line>
      <Line x={W - 10} y={13} c={sc.accent} size={9} anchor="end">{`${Math.round(s.miles)}/${TRAIL_MILES} mi`}</Line>
      <rect x={barX} y={barY} width={barW} height={4} fill={sc.frame} opacity={0.3} />
      <rect x={barX} y={barY} width={barW * frac} height={4} fill={sc.good} opacity={0.65} />
      {LANDMARKS.map((lm) => {
        const x = Math.round(barX + (barW * lm.mile) / TRAIL_MILES);
        const behind = s.miles >= lm.mile;
        const c = behind ? sc.dim : sc.accent;
        const o = behind ? 0.6 : 1;
        if (lm.type === "fort") return <rect key={lm.name} x={x - 1} y={barY + 0.5} width={3} height={3} fill={c} opacity={o} />;
        if (lm.type === "river")
          return (
            <g key={lm.name} opacity={o}>
              <rect x={x - 1} y={barY + 2} width={1} height={1} fill={c} />
              <rect x={x} y={barY + 1} width={1} height={1} fill={c} />
              <rect x={x + 1} y={barY + 2} width={1} height={1} fill={c} />
            </g>
          );
        if (lm.type === "end") return <rect key={lm.name} x={x - 1} y={barY - 1} width={2} height={6} fill={c} opacity={o} />;
        return <rect key={lm.name} x={x} y={barY} width={1} height={4} fill={c} opacity={o} />;
      })}
      <rect x={wagonX - 2} y={barY - 2} width={5} height={4} fill={sc.fg} />
      <rect x={wagonX - 3} y={barY} width={1} height={2} fill={sc.fg} />
      <rect x={wagonX + 2} y={barY} width={1} height={2} fill={sc.fg} />
    </g>
  );
}

function StoreScreen(s: OregonState, sc: ScreenColors) {
  const spent = s.spend.reduce((a, b) => a + b, 0);
  const left = s.budget - spent;
  const units = [
    `${Math.round(s.spend[0] / OX_COST)} teams`,
    `${s.spend[1] * FOOD_PER_$} lbs`,
    `${s.spend[2] * AMMO_PER_$} rounds`,
    `${Math.round(s.spend[3] / CLOTH_COST)} sets`,
    `${Math.round(s.spend[4] / MISC_COST)} kits`,
  ];
  const title = s.storeMode === "outfit" ? "MATT'S GENERAL STORE" : "TRADING POST";
  return (
    <g>
      <PxText x={W / 2} y={16} size={12} fill={sc.accent} shadow={sc.bg} anchor="middle">{title}</PxText>
      <Line x={W / 2} y={30} c={sc.dim} size={8} anchor="middle">{`You have $${left} to spend`}</Line>
      {STORE_ROWS.map((row, i) => {
        const y = 46 + i * 13;
        const sel = i === s.cursor;
        return (
          <g key={row}>
            {sel && <Line x={10} y={y} c={sc.accent} size={8}>▶</Line>}
            <Line x={22} y={y} c={sel ? sc.fg : sc.dim} size={8}>{row.padEnd(12)}</Line>
            <Line x={130} y={y} c={sel ? sc.fg : sc.dim} size={8}>{`$${s.spend[i]}`.padStart(5)}</Line>
            <Line x={172} y={y} c={sc.dim} size={8}>{`(${units[i]})`}</Line>
          </g>
        );
      })}
      <Line x={W / 2} y={H - 6} c={sc.dim} size={7} anchor="middle">
        {s.storeMode === "outfit" ? "↑↓ pick   ←→ spend   ⏎ hit the trail" : "↑↓ pick   ←→ spend   ⏎ done trading"}
      </Line>
    </g>
  );
}

/** One traveler as `Name ▮▮▮▯`: four blocks of health, red when ailing. */
function MemberRow({ m, x, y, sc }: { m: Member; x: number; y: number; sc: ScreenColors }) {
  const blocks = m.alive ? Math.max(m.health > 0 ? 1 : 0, Math.round(m.health / 25)) : 0;
  const color = !m.alive ? sc.dim : m.ailment ? sc.bad : m.health > 50 ? sc.good : sc.accent;
  return (
    <g>
      <Line x={x} y={y} c={m.alive ? sc.fg : sc.dim} size={7}>{m.alive ? shortName(m.name) : `†${shortName(m.name)}`}</Line>
      {[0, 1, 2, 3].map((i) => (
        <rect key={i} x={x + 30 + i * 4} y={y - 5} width={3} height={4} fill={color} opacity={i < blocks ? 1 : 0.25} />
      ))}
    </g>
  );
}

function TrailScreen(s: OregonState, sc: ScreenColors) {
  const paceNote = ["+0 mi", `+${PACE_MILES[1]} mi`, `+${PACE_MILES[2]} mi`][s.pace];
  const paceHealth = ["health ↑", "health ↓", "health ↓↓"][s.pace];
  const rationNote = `${RATION_LB[s.rations] * aliveCount(s)} lb/leg`;
  const rationHealth = ["health ↑", "health ↓", "health ↓↓"][s.rations];
  const menu = [
    "1  Continue on the trail",
    "2  Hunt for food",
    `3  Pace: ${PACE_NAMES[s.pace]}`.padEnd(22) + `${paceNote} · ${paceHealth}`,
    `4  Rations: ${RATION_NAMES[s.rations]}`.padEnd(22) + `${rationNote} · ${rationHealth}`,
  ];
  if (s.atFort) menu.push(`5  Trade for supplies`);
  const stats = [
    `Food ${Math.round(s.food)} lb`.padEnd(16) + `Oxen ${s.oxen}`.padEnd(10) + `Health: ${healthWord(s.health)}`,
    `Bullets ${s.bullets}`.padEnd(16) + `Clothes ${s.clothing}`.padEnd(10) + `Weather: ${s.weather}`,
    `Medicine ${s.misc}`.padEnd(16) + `Cash $${Math.round(s.cash)}`,
  ];
  const leg = s.lastLeg;
  const ailing = s.party.filter((m) => m.alive && m.ailment);
  return (
    <g>
      {Header(s, sc)}
      {leg ? (
        <Line x={10} y={33} c={sc.dim} size={7}>{`Last leg: ${signed(Math.round(leg.miles))} mi · −${Math.round(leg.ate)} lb food · health ${signed(leg.dh)}`}</Line>
      ) : (
        <Line x={10} y={33} c={sc.dim} size={7}>{`${OCCUPATIONS[s.occupation].name}'s wagon, ready to roll`}</Line>
      )}
      {s.party.map((m, i) => (
        <MemberRow key={m.name} m={m} x={10 + i * 54} y={44} sc={sc} />
      ))}
      {ailing.length > 0 && (
        <Line x={10} y={53} c={sc.bad} size={7}>{ailing.map((m) => `${shortName(m.name)}: ${m.ailment}`).join("  ")}</Line>
      )}
      {stats.map((line, i) => (
        <Line key={i} x={10} y={62 + i * 9} c={s.health > 50 || i !== 0 ? sc.fg : sc.bad} size={8}>{line}</Line>
      ))}
      {menu.map((line, i) => (
        <Line key={line} x={12} y={MENU_TOP + i * MENU_STEP} c={sc.accent} size={8}>{line}</Line>
      ))}
      {!s.atFort && <Line x={W - 10} y={H - 5} c={sc.dim} size={7} anchor="end">esc makes camp</Line>}
    </g>
  );
}

function HuntScreen(s: OregonState, sc: ScreenColors, p: ThemePalette) {
  if (!s.hunt) return <g />;
  return (
    <g>
      <HuntScene t={s.hunt} p={p} />
      <Px x={0} y={ROWS - 3} w={COLS} h={3} fill="#000" o={0.45} />
      <Line x={W / 2} y={H - 4} c={sc.fg} size={7} anchor="middle">
        {s.hunt.done ? "press any key to head back to the wagon" : "← ↑ ↓ → walk · space fires · 100 lb carry limit"}
      </Line>
    </g>
  );
}

function RiverScreen(s: OregonState, sc: ScreenColors) {
  return (
    <g>
      {Header(s, sc)}
      <PxText x={W / 2} y={44} size={11} fill={sc.accent} shadow={sc.bg} anchor="middle">{`THE ${s.riverName.toUpperCase()}`}</PxText>
      <Line x={W / 2} y={58} c={sc.dim} size={8} anchor="middle">The river blocks the trail. How</Line>
      <Line x={W / 2} y={68} c={sc.dim} size={8} anchor="middle">will you get the wagon across?</Line>
      {["1  Ford the river", "2  Caulk the wagon and float", "3  Wait for conditions to improve", `4  Take the ferry ($${FERRY_COST})`].map((line, i) => (
        <Line key={line} x={20} y={RIVER_TOP + i * RIVER_STEP} c={i === 3 && s.cash < FERRY_COST ? sc.dim : sc.accent} size={8}>{line}</Line>
      ))}
    </g>
  );
}

function MessageScreen(s: OregonState, sc: ScreenColors) {
  const tone = s.msgTone === "good" ? sc.good : s.msgTone === "bad" ? sc.bad : sc.accent;
  return (
    <g>
      {Header(s, sc)}
      <rect x={16} y={38} width={W - 32} height={72} fill="#000" opacity={0.5} />
      <rect x={16} y={38} width={W - 32} height={72} fill="none" stroke={tone} strokeWidth={1} opacity={0.8} />
      <PxText x={W / 2} y={58} size={11} fill={tone} shadow={sc.bg} anchor="middle">{s.msgTitle}</PxText>
      {s.msgLines.map((line, i) => (
        <Line key={i} x={W / 2} y={74 + i * 11} c={sc.fg} size={8} anchor="middle">{line}</Line>
      ))}
      <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">press any key to continue</Line>
    </g>
  );
}

function ChoiceScreen(s: OregonState, sc: ScreenColors) {
  return (
    <g>
      {Header(s, sc)}
      <rect x={16} y={36} width={W - 32} height={82} fill="#000" opacity={0.5} />
      <rect x={16} y={36} width={W - 32} height={82} fill="none" stroke={sc.accent} strokeWidth={1} opacity={0.8} />
      <PxText x={W / 2} y={54} size={11} fill={sc.accent} shadow={sc.bg} anchor="middle">{s.msgTitle}</PxText>
      {s.msgLines.map((line, i) => (
        <Line key={i} x={W / 2} y={68 + i * 11} c={sc.fg} size={8} anchor="middle">{line}</Line>
      ))}
      <Line x={40} y={100} c={sc.good} size={8}>{`1  ${s.choiceOptions[0]}`}</Line>
      <Line x={W / 2 + 10} y={100} c={sc.good} size={8}>{`2  ${s.choiceOptions[1]}`}</Line>
      <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">what do you do?</Line>
    </g>
  );
}

function OccupationScreen(s: OregonState, sc: ScreenColors) {
  return (
    <g>
      <PxText x={W / 2} y={18} size={12} fill={sc.accent} shadow={sc.bg} anchor="middle">WHO ARE YOU?</PxText>
      <Line x={W / 2} y={32} c={sc.dim} size={8} anchor="middle">Less money means a harder trail</Line>
      <Line x={W / 2} y={42} c={sc.dim} size={8} anchor="middle">— and a bigger score if you make it.</Line>
      {OCCUPATIONS.map((o, i) => (
        <g key={o.name}>
          <Line x={22} y={62 + i * 16} c={sc.accent} size={9}>{`${i + 1}  ${o.name}`}</Line>
          <Line x={110} y={62 + i * 16} c={sc.fg} size={8}>{o.blurb}</Line>
        </g>
      ))}
      {s.best > 0 && (
        <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">{`best score ${s.best}`}</Line>
      )}
    </g>
  );
}

const rankWord = (rank: number) => (rank === 1 ? "#1 of all time!" : `#${rank} of all time`);

function OverScreen(s: OregonState, sc: ScreenColors) {
  if (s.arrived) {
    return (
      <g>
        <PxText x={W / 2} y={30} size={15} fill={sc.good} shadow={sc.bg} anchor="middle">{s.newBest ? "NEW BEST!" : "YOU MADE IT!"}</PxText>
        <Line x={W / 2} y={52} c={sc.fg} size={9} anchor="middle">{`Oregon City in ${s.day} days as a ${OCCUPATIONS[s.occupation].name.toLowerCase()}`}</Line>
        <Line x={W / 2} y={66} c={sc.fg} size={9} anchor="middle">{`${aliveCount(s)} of 5 survived the trail`}</Line>
        <PxText x={W / 2} y={92} size={13} fill={sc.accent} shadow={sc.bg} anchor="middle">{`SCORE ${s.score}   BEST ${s.best}`}</PxText>
        {s.rank > 0 && <Line x={W / 2} y={108} c={sc.good} size={8} anchor="middle">{rankWord(s.rank)}</Line>}
        <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">press any key to travel again</Line>
      </g>
    );
  }
  // Tombstone.
  const stoneW = 92;
  const stoneX = (W - stoneW) / 2;
  return (
    <g>
      <path
        d={`M ${stoneX} ${H} L ${stoneX} 52 Q ${stoneX} 30 ${W / 2} 30 Q ${stoneX + stoneW} 30 ${stoneX + stoneW} 52 L ${stoneX + stoneW} ${H} Z`}
        fill={sc.dim}
        opacity={0.25}
        stroke={sc.frame}
        strokeWidth={1}
      />
      <Line x={W / 2} y={52} c={sc.fg} size={8} anchor="middle">HERE LIES</Line>
      <PxText x={W / 2} y={70} size={11} fill={sc.fg} shadow={sc.bg} anchor="middle">{s.epitaph.toUpperCase()}</PxText>
      <Line x={W / 2} y={86} c={sc.dim} size={8} anchor="middle">{`died of ${s.cause}`}</Line>
      <Line x={W / 2} y={102} c={sc.accent} size={8} anchor="middle">{`Score ${s.score}   Best ${s.best}`}</Line>
      {s.rank > 0 && <Line x={W / 2} y={114} c={sc.good} size={7} anchor="middle">{rankWord(s.rank)}</Line>}
      <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">press any key to travel again</Line>
    </g>
  );
}

function OregonRender(s: OregonState, p: ThemePalette) {
  const sc = screenColors(p);
  let body: React.JSX.Element;
  let label: string;
  switch (s.phase) {
    case "occupation":
      body = OccupationScreen(s, sc);
      label = "The Oxen Trail: choose your occupation";
      break;
    case "choice":
      body = ChoiceScreen(s, sc);
      label = `The Oxen Trail: ${s.msgTitle} — press 1 or 2`;
      break;
    case "store":
      body = StoreScreen(s, sc);
      label = "The Oxen Trail: outfit your wagon at the general store";
      break;
    case "hunt":
      body = HuntScreen(s, sc, p);
      label = "The Oxen Trail: hunting — walk with the arrows and fire with space";
      break;
    case "river":
      body = RiverScreen(s, sc);
      label = "The Oxen Trail: a river crossing";
      break;
    case "message":
      body = MessageScreen(s, sc);
      label = `The Oxen Trail: ${s.msgTitle}`;
      break;
    case "over":
      body = OverScreen(s, sc);
      label = s.arrived ? "The Oxen Trail: you reached Oregon" : "The Oxen Trail: the party has perished";
      break;
    default:
      body = TrailScreen(s, sc);
      label = "The Oxen Trail: on the trail — choose your next move";
  }
  return (
    <GameFrame label={label}>
      <Px x={0} y={0} w={COLS} h={ROWS} fill={sc.bg} />
      <rect x={0} y={0} width={W} height={2} fill={sc.accent} opacity={0.5} />
      {body}
    </GameFrame>
  );
}

/** Attract screen: a scenic title card — wagon on the plains under mountains,
    the all-time table on the right, and last run's grave by the trail. */
function OregonAttract(p: ThemePalette) {
  const c = sceneColors(p);
  const hz = 20;
  const table = loadTable().slice(0, 3);
  const tomb = loadTomb();
  return (
    <GameFrame label="The Oxen Trail title screen: a wagon bound for Oregon">
      <Px x={0} y={0} w={COLS} h={ROWS} fill={c.sky} />
      <Px x={58} y={3} w={5} h={5} fill={c.sun} />
      {poly([[0, hz], [12, 12], [24, hz - 2], [36, 9], [48, hz - 2], [60, 13], [72, hz - 1], [72, hz]], c.mountain)}
      <Px x={0} y={hz} w={COLS} h={ROWS - hz} fill={c.grass} />
      {poly([[30, hz], [42, hz], [56, ROWS], [16, ROWS]], c.trail, 0.5)}
      {/* wagon + ox team */}
      {(() => {
        const x = 30;
        const y = 24;
        const bars = [];
        for (let i = 0; i <= 10; i++) {
          const top = 4 - Math.round(4 * Math.sin((Math.PI * i) / 10));
          bars.push(<Px key={i} x={x + i} y={y + top} w={1} h={4 - top} fill={c.snow} />);
        }
        return (
          <g>
            {bars}
            <Px x={x} y={y + 4} w={11} h={2} fill={c.mountain} />
            <Px x={x + 1} y={y + 6} w={2} h={1} fill={c.line} />
            <Px x={x + 8} y={y + 6} w={2} h={1} fill={c.line} />
            {/* oxen with realistic horns — light base, dark tips curving up */}
            <Px x={x + 11} y={y + 1} w={2} h={1} fill={c.ox} />
            <Px x={x + 13} y={y + 1} w={2} h={1} fill={c.ox} />
            <Px x={x + 10} y={y + 0} w={1} h={1} fill={c.line} />
            <Px x={x + 15} y={y + 0} w={1} h={1} fill={c.line} />
            <Px x={x + 13} y={y + 2} w={5} h={3} fill={c.ox} />
            <Px x={x + 12} y={y + 2} w={1} h={2} fill={c.ox} />
          </g>
        );
      })()}
      {/* last run's grave, off the trail to the left */}
      {tomb && (
        <g>
          <Px x={6} y={25} w={7} h={7} fill={c.mountainBack} />
          <Px x={7} y={24} w={5} h={1} fill={c.mountainBack} />
          <Px x={7} y={26} w={5} h={1} fill={c.sky} o={0.5} />
          <Line x={9.5 * U} y={32.4 * U} c={c.text} size={5} anchor="middle">{`RIP ${shortName(tomb.name)}`}</Line>
        </g>
      )}
      {/* the all-time table */}
      <g>
        <Line x={W - 8} y={19 * U} c={c.accent} size={6} anchor="end">BEST TRAILS</Line>
        {table.map((e, i) => (
          <Line key={i} x={W - 8} y={(21.5 + i * 2) * U} c={e.name === "You" ? c.accent : c.text} size={5.5} anchor="end">{`${e.name} ${e.score}`}</Line>
        ))}
      </g>
      <PxText x={(COLS * U) / 2} y={7 * U} size={15} fill={c.accent} shadow="#000" anchor="middle">THE OXEN TRAIL</PxText>
      <PxText x={(COLS * U) / 2} y={11 * U} size={7} fill={c.text} shadow="#000" anchor="middle">Guide your wagon to Oregon</PxText>
    </GameFrame>
  );
}

export const OxenTrailGame: HeroGameDefinition<OregonState> = {
  title: "The Oxen Trail",
  tab: "Trail",
  seedSalt: 0x0a11,
  initialState: () => freshGame(),
  onStart: (s) => freshGame(Math.max(s.best, loadBest("trail"))),
  handleKey: oregonKey,
  handleKeyUp: oregonKeyUp,
  handlePointer: oregonPointer,
  onPause: (s) => (s.phase === "hunt" && s.hunt ? { ...s, hunt: tripPause(s.hunt) } : s),
  update: oregonUpdate,
  render: OregonRender,
  renderAttract: OregonAttract,
  keys: (key) => key.length === 1 || ["Enter", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(key),
};
