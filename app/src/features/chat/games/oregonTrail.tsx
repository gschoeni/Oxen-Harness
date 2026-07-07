// The Oxen Trail — a compact homage to the original 1978 Oregon Trail text game.
// You outfit a wagon at the general store, then play out the journey two weeks at
// a time: continue, hunt, and set your pace and rations while random events,
// river crossings, illness, and weather whittle down your party. Reach Oregon
// City before everyone dies. Rendered as a retro green-screen terminal, driven
// entirely by the number keys (menus) and the keyboard (the hunting mini-game).
//
// It plugs into the same HeroGameDefinition contract as the arcade game, so the
// hero can swap between the two cabinets. Because it's turn-based, `update` only
// advances a clock (for the blinking caret and the hunt timer); all real state
// changes happen in `handleKey`.

import type { ThemePalette } from "../../../lib/types";
import { clamp, COLS, GameFrame, Px, poly, PxText, ROWS, sceneColors, U, type HeroGameDefinition } from "./gameKit";

type Phase = "store" | "trail" | "hunt" | "message" | "river" | "over";
type Tone = "good" | "bad" | "neutral";

interface Member {
  name: string;
  alive: boolean;
}

interface OregonState {
  phase: Phase;
  storeMode: "outfit" | "fort";
  clock: number; // seconds since play began — caret blink + hunt timer

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
  health: number; // 0..100, higher is better
  party: Member[];
  nextLandmark: number;
  atFort: boolean;
  weather: string;

  // message card
  msgTitle: string;
  msgLines: string[];
  msgTone: Tone;
  afterMessage: Phase;

  // river crossing
  riverName: string;

  // hunting mini-game
  huntWord: string;
  huntTyped: string;
  huntStart: number;

  // outcome
  arrived: boolean;
  cause: string;
  epitaph: string;
  score: number;
}

const TRAIL_MILES = 2040;
const PARTY_NAMES = ["Wagon Boss", "Sarah", "Charlie", "Hank", "Milly"];
const STORE_ROWS = ["Oxen team", "Food", "Ammunition", "Clothing", "Supplies"];
const HUNT_WORDS = ["BANG", "POW", "WHAM", "BLAM", "ZAP"];
const HUNT_LIMIT = 4.2; // seconds before the game gets away
const CAUSES = ["dysentery", "typhoid fever", "cholera", "measles", "exhaustion", "a fever", "a snakebite"];

const PACE_NAMES = ["Steady", "Strenuous", "Grueling"];
const RATION_NAMES = ["Filling", "Meager", "Bare bones"];
const RATION_LB = [55, 38, 24]; // lbs the party eats per leg
const PACE_MILES = [0, 24, 46];
const PACE_H = [4, -3, -10];
const RATION_H = [3, -3, -11];

// Costs turning dollars into units when you leave a store screen.
const OX_COST = 40;
const FOOD_PER_$ = 5;
const AMMO_PER_$ = 50;
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

const ri = (a: number, b: number) => a + Math.floor(Math.random() * (b - a + 1));
const pick = <T,>(arr: T[]) => arr[Math.floor(Math.random() * arr.length)];
const aliveCount = (s: OregonState) => s.party.filter((m) => m.alive).length;
const healthWord = (h: number) => (h > 75 ? "Good" : h > 50 ? "Fair" : h > 25 ? "Poor" : "Very poor");

function freshGame(): OregonState {
  return {
    phase: "store",
    storeMode: "outfit",
    clock: 0,
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
    party: PARTY_NAMES.map((name) => ({ name, alive: true })),
    nextLandmark: 0,
    atFort: false,
    weather: "Fair",
    msgTitle: "",
    msgLines: [],
    msgTone: "neutral",
    afterMessage: "trail",
    riverName: "",
    huntWord: "",
    huntTyped: "",
    huntStart: 0,
    arrived: false,
    cause: "",
    epitaph: "",
    score: 0,
  };
}

function toMessage(s: OregonState, title: string, lines: string[], tone: Tone, after: Phase): OregonState {
  return { ...s, phase: "message", msgTitle: title, msgLines: lines, msgTone: tone, afterMessage: after };
}

function scoreOf(s: OregonState): number {
  return (
    aliveCount(s) * 350 +
    Math.floor(s.cash) +
    Math.floor(s.food / 8) +
    s.oxen * 4 +
    s.misc * 3 +
    s.clothing * 3 +
    Math.round(s.health * 2)
  );
}

// ---- random events ---------------------------------------------------------
// Each event returns the message to show and the fields it changes. Absolute
// values keep them easy to read; health is clamped by the caller.

interface EventResult {
  title: string;
  lines: string[];
  tone: Tone;
  changes: Partial<OregonState>;
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
      changes: { misc: Math.max(0, s.misc - 1), day: s.day + (s.misc > 0 ? 1 : 3), health: s.health - (s.misc > 0 ? 2 : 8) },
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
      const spent = ri(15, 35);
      return {
        title: "Wild animals attack",
        lines: [`You drive them off, spending`, `${spent} bullets in the night.`],
        tone: "bad",
        changes: { bullets: Math.max(0, s.bullets - spent), health: s.health - (s.bullets < spent ? 10 : 0) },
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      const stolen = ri(60, 160);
      return {
        title: "Bandits attack!",
        lines: [`They make off with about`, `${stolen} lbs of your food.`],
        tone: "bad",
        changes: { food: Math.max(0, s.food - stolen), bullets: Math.max(0, s.bullets - ri(10, 30)) },
      };
    },
  },
  {
    weight: 2,
    run: (s) => ({
      title: "Unsafe water",
      lines: ["Bad water sickens the party.", "You lose time finding a spring."],
      tone: "bad",
      changes: { day: s.day + 1, health: s.health - 10 },
    }),
  },
  {
    weight: 2,
    run: (s) => ({
      title: "Heavy rains",
      lines: ["The trail turns to mud and", "some of your food spoils."],
      tone: "bad",
      changes: { day: s.day + 1, food: Math.max(0, s.food - ri(20, 45)) },
    }),
  },
  {
    weight: 2,
    run: (s) => ({
      title: "Hail storm",
      lines: ["Hail hammers the wagon and", "damages your supplies."],
      tone: "bad",
      changes: { misc: Math.max(0, s.misc - 1), food: Math.max(0, s.food - ri(10, 30)) },
    }),
  },
  {
    weight: 1,
    run: (s) => ({
      title: "Fire in the wagon!",
      lines: ["You lose food and supplies", "to the flames."],
      tone: "bad",
      changes: { food: Math.max(0, s.food - ri(30, 70)), misc: Math.max(0, s.misc - 1) },
    }),
  },
  {
    weight: 2,
    run: (s) => {
      const victim = pick(s.party.filter((m) => m.alive));
      const treated = s.misc > 0;
      return {
        title: "Snakebite!",
        lines: treated ? [`A rattlesnake bites ${victim.name}.`, "Your medicine pulls them through."] : [`A rattlesnake bites ${victim.name}`, "and you have no medicine."],
        tone: "bad",
        changes: { misc: Math.max(0, s.misc - 1), health: s.health - (treated ? 6 : 22) },
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      const treated = s.misc > 0;
      return {
        title: "Dysentery strikes",
        lines: treated ? ["Illness spreads through camp,", "but your medicine holds it off."] : ["Illness spreads and you have", "no medicine to treat it."],
        tone: "bad",
        changes: { misc: Math.max(0, s.misc - 1), health: s.health - (treated ? 8 : 26) },
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
        changes: { health: s.health - (clothed ? 3 : 15), weather: "Cold" },
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      const found = ri(30, 70);
      return {
        title: "Helpful travelers",
        lines: [`Friendly travelers share the`, `way to ${found} lbs of food.`],
        tone: "good",
        changes: { food: s.food + found },
      };
    },
  },
  {
    weight: 2,
    run: (s) => {
      const found = ri(15, 40);
      return {
        title: "Wild fruit!",
        lines: [`You gather ${found} lbs of berries`, "along the trail."],
        tone: "good",
        changes: { food: s.food + found, health: s.health + 4 },
      };
    },
  },
  {
    weight: 1,
    run: (s) => ({
      title: "Good trail",
      lines: ["The trail is dry and clear.", "Spirits lift as you roll on."],
      tone: "good",
      changes: { health: s.health + 6 },
    }),
  },
];

function rollEvent(s: OregonState): EventResult | null {
  const pool = EVENTS.filter((e) => !e.when || e.when(s));
  const total = pool.reduce((sum, e) => sum + e.weight, 0);
  let r = Math.random() * total;
  for (const e of pool) {
    r -= e.weight;
    if (r <= 0) return e.run(s);
  }
  return null;
}

// ---- outcomes --------------------------------------------------------------

function arrive(s: OregonState): OregonState {
  const next = { ...s, miles: TRAIL_MILES, arrived: true };
  next.score = scoreOf(next);
  return toMessage(next, "OREGON CITY!", [`You reached Oregon in ${next.day} days`, `with ${aliveCount(next)} of 5 still standing.`, `Final score: ${next.score}`], "good", "over");
}

// Health has bottomed out: the weakest member is lost. When the last one goes,
// the run is over (the classic tombstone).
function afflict(s: OregonState): OregonState {
  const living = s.party.filter((m) => m.alive);
  const victim = living[living.length - 1];
  const party = s.party.map((m) => (m.name === victim.name ? { ...m, alive: false } : m));
  const cause = pick(CAUSES);
  const next = { ...s, party };
  if (party.some((m) => m.alive)) {
    next.health = 45;
    next.misc = Math.max(0, s.misc - 1);
    return toMessage(next, `${victim.name} has died`, [`of ${cause}.`, "The rest of the party carries on."], "bad", "trail");
  }
  next.arrived = false;
  next.cause = cause;
  next.epitaph = victim.name;
  next.score = scoreOf(next);
  return { ...next, phase: "over" };
}

// One two-week leg of travel: eat, advance, weather, health, then whatever the
// trail throws at you (arrival, a landmark, an event, or a quiet stretch).
function travel(s: OregonState): OregonState {
  const alive = aliveCount(s);
  const eat = alive * RATION_LB[s.rations];
  const starving = s.food < eat;
  const food = Math.max(0, s.food - eat);

  const landmark = LANDMARKS[s.nextLandmark];
  let base = 90 + s.oxen * 6 + PACE_MILES[s.pace] + ri(0, 26);
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

  const cold = miles > 950 && Math.random() < 0.5;
  const weather = miles > 1500 ? (cold ? "Snow" : "Cold") : cold ? "Cold" : pick(["Fair", "Clear", "Rainy", "Windy"]);

  let dh = PACE_H[s.pace] + RATION_H[s.rations];
  if (starving) dh -= 22;
  if (cold && s.clothing < alive) dh -= 14;

  let next: OregonState = {
    ...s,
    food,
    miles,
    day: s.day + 14,
    weather,
    health: clamp(s.health + dh, 0, 100),
    atFort: false,
  };

  // A quiet leg can still spring an event (never on the same leg you reach a
  // landmark — that gets the spotlight).
  let eventMsg: EventResult | null = null;
  if (!reached && Math.random() < 0.45) {
    eventMsg = rollEvent(next);
    if (eventMsg) next = { ...next, ...eventMsg.changes, health: clamp((eventMsg.changes.health ?? next.health) as number, 0, 100) };
  }

  if (next.miles >= TRAIL_MILES) return arrive(next);
  if (next.health <= 0) return afflict(next);

  if (reached) {
    next = { ...next, nextLandmark: s.nextLandmark + 1 };
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
  const deep = Math.random();

  if (choice === 3) {
    // Wait for better conditions, then cross safely.
    return toMessage({ ...next, day: s.day + ri(1, 3) }, `You wait at the ${river}`, ["The waters calm after a few days", "and you cross without trouble."], "neutral", "trail");
  }

  const ford = choice === 1;
  const disaster = ford ? deep > 0.55 : deep > 0.78; // fording is riskier

  if (!disaster) {
    return toMessage(next, `You cross the ${river}`, ford ? ["You ford the river and reach", "the far bank safely."] : ["You caulk the wagon and float", "across without a hitch."], "good", "trail");
  }

  // Something goes wrong in the water.
  if (Math.random() < 0.4) {
    return afflict({ ...next, health: 0 }); // a drowning
  }
  const lostFood = ri(80, 200);
  const lostOx = ford && s.oxen > 1 && Math.random() < 0.5 ? 1 : 0;
  return toMessage(
    { ...next, food: Math.max(0, s.food - lostFood), oxen: s.oxen - lostOx, health: clamp(s.health - 12, 0, 100) },
    `The ${river} nearly takes you`,
    [`The wagon tips and you lose`, `${lostFood} lbs of food${lostOx ? " and an ox" : ""}.`],
    "bad",
    "trail",
  );
}

// ---- hunting mini-game -----------------------------------------------------

function startHunt(s: OregonState): OregonState {
  if (s.bullets < 10) return toMessage(s, "Out of bullets", ["You need more ammunition", "before you can hunt."], "bad", "trail");
  return { ...s, phase: "hunt", huntWord: pick(HUNT_WORDS), huntTyped: "", huntStart: s.clock };
}

function resolveHunt(s: OregonState, hit: boolean): OregonState {
  const elapsed = s.clock - s.huntStart;
  const bullets = Math.max(0, s.bullets - ri(10, 20));
  if (!hit) return toMessage({ ...s, bullets }, "The game got away", ["Your shot went wide and the", "animals scattered."], "bad", "trail");
  const speed = clamp(1 - elapsed / HUNT_LIMIT, 0.15, 1);
  const meat = Math.round((45 + Math.random() * 55) * speed);
  return toMessage({ ...s, bullets, food: s.food + meat }, "A clean shot!", [`You bring back ${meat} lbs`, "of fresh meat to the wagon."], "good", "trail");
}

// ---- input -----------------------------------------------------------------

function storeKey(s: OregonState, key: string): OregonState {
  if (key === "ArrowUp") return { ...s, cursor: (s.cursor + STORE_ROWS.length - 1) % STORE_ROWS.length };
  if (key === "ArrowDown") return { ...s, cursor: (s.cursor + 1) % STORE_ROWS.length };

  const spent = s.spend.reduce((a, b) => a + b, 0);
  if (key === "ArrowLeft") {
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
  if (key === "Backspace") return { ...s, huntTyped: s.huntTyped.slice(0, -1) };
  if (key.length !== 1) return s;
  const ch = key.toUpperCase();
  const expected = s.huntWord[s.huntTyped.length];
  if (ch !== expected) return s; // a miss-key simply doesn't register
  const typed = s.huntTyped + ch;
  if (typed === s.huntWord) return resolveHunt(s, true);
  return { ...s, huntTyped: typed };
}

function oregonKey(s: OregonState, key: string): OregonState {
  switch (s.phase) {
    case "store":
      return storeKey(s, key);
    case "trail":
      return trailKey(s, key);
    case "hunt":
      return huntKey(s, key);
    case "message":
      return { ...s, phase: s.afterMessage };
    case "river":
      if (key === "1" || key === "2" || key === "3") return crossRiver(s, Number(key));
      return s;
    case "over":
      return freshGame();
    default:
      return s;
  }
}

function oregonUpdate(s: OregonState, dt: number): OregonState {
  const clock = s.clock + dt;
  if (s.phase === "hunt" && clock - s.huntStart > HUNT_LIMIT) return resolveHunt({ ...s, clock }, false);
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

/** The status header with a progress bar and a wagon inching toward Oregon. */
function Header(s: OregonState, sc: ScreenColors) {
  const frac = clamp(s.miles / TRAIL_MILES, 0, 1);
  const barX = 10;
  const barW = W - 20;
  const wagonX = barX + barW * frac;
  return (
    <g>
      <Line x={10} y={13} c={sc.fg} size={9}>{`Day ${s.day}`}</Line>
      <Line x={W / 2} y={13} c={sc.dim} size={9} anchor="middle">{dateStr(s.day)}</Line>
      <Line x={W - 10} y={13} c={sc.accent} size={9} anchor="end">{`${Math.round(s.miles)}/${TRAIL_MILES} mi`}</Line>
      <rect x={barX} y={20} width={barW} height={4} fill={sc.frame} opacity={0.3} />
      <rect x={barX} y={20} width={barW * frac} height={4} fill={sc.good} opacity={0.65} />
      <rect x={wagonX - 2} y={18} width={5} height={4} fill={sc.fg} />
      <rect x={wagonX - 3} y={20} width={1} height={2} fill={sc.fg} />
      <rect x={wagonX + 2} y={20} width={1} height={2} fill={sc.fg} />
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

function TrailScreen(s: OregonState, sc: ScreenColors) {
  const menu = [
    "1  Continue on the trail",
    "2  Hunt for food",
    `3  Pace: ${PACE_NAMES[s.pace]}`,
    `4  Rations: ${RATION_NAMES[s.rations]}`,
  ];
  if (s.atFort) menu.push(`5  Trade for supplies`);
  const stats = [
    `Food ${Math.round(s.food)} lb`.padEnd(16) + `Oxen ${s.oxen}`,
    `Bullets ${s.bullets}`.padEnd(16) + `Clothes ${s.clothing}`,
    `Medicine ${s.misc}`.padEnd(16) + `Cash $${Math.round(s.cash)}`,
  ];
  return (
    <g>
      {Header(s, sc)}
      <Line x={10} y={40} c={sc.dim} size={8}>{`Party ${aliveCount(s)}/5`}</Line>
      <Line x={W / 2} y={40} c={s.health > 50 ? sc.good : sc.bad} size={8} anchor="middle">{`Health: ${healthWord(s.health)}`}</Line>
      <Line x={W - 10} y={40} c={sc.dim} size={8} anchor="end">{`${s.weather}`}</Line>
      {stats.map((line, i) => (
        <Line key={i} x={10} y={54 + i * 10} c={sc.fg} size={8}>{line}</Line>
      ))}
      {menu.map((line, i) => (
        <Line key={line} x={12} y={88 + i * 10} c={sc.accent} size={8}>{line}</Line>
      ))}
      <Line x={W - 10} y={H - 5} c={sc.dim} size={7} anchor="end">esc makes camp</Line>
    </g>
  );
}

function HuntScreen(s: OregonState, sc: ScreenColors) {
  const elapsed = s.clock - s.huntStart;
  const frac = clamp(1 - elapsed / HUNT_LIMIT, 0, 1);
  const caret = Math.floor(s.clock * 2) % 2 === 0 ? "_" : " ";
  return (
    <g>
      <PxText x={W / 2} y={26} size={12} fill={sc.accent} shadow={sc.bg} anchor="middle">HUNTING</PxText>
      <Line x={W / 2} y={42} c={sc.dim} size={8} anchor="middle">Type the word — fast!</Line>
      <PxText x={W / 2} y={78} size={22} fill={sc.fg} shadow={sc.bg} anchor="middle">{s.huntWord}</PxText>
      <Line x={W / 2} y={98} c={sc.good} size={12} anchor="middle">{s.huntTyped + caret}</Line>
      <rect x={40} y={112} width={W - 80} height={4} fill={sc.frame} opacity={0.3} />
      <rect x={40} y={112} width={(W - 80) * frac} height={4} fill={frac > 0.35 ? sc.good : sc.bad} />
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
      {["1  Ford the river", "2  Caulk the wagon and float", "3  Wait for conditions to improve"].map((line, i) => (
        <Line key={line} x={20} y={90 + i * 12} c={sc.accent} size={8}>{line}</Line>
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

function OverScreen(s: OregonState, sc: ScreenColors) {
  if (s.arrived) {
    return (
      <g>
        <PxText x={W / 2} y={34} size={15} fill={sc.good} shadow={sc.bg} anchor="middle">YOU MADE IT!</PxText>
        <Line x={W / 2} y={58} c={sc.fg} size={9} anchor="middle">{`Oregon City in ${s.day} days`}</Line>
        <Line x={W / 2} y={74} c={sc.fg} size={9} anchor="middle">{`${aliveCount(s)} of 5 survived the trail`}</Line>
        <PxText x={W / 2} y={100} size={13} fill={sc.accent} shadow={sc.bg} anchor="middle">{`SCORE ${s.score}`}</PxText>
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
      <Line x={W / 2} y={102} c={sc.accent} size={8} anchor="middle">{`Score ${s.score}`}</Line>
      <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">press any key to travel again</Line>
    </g>
  );
}

function OregonRender(s: OregonState, p: ThemePalette) {
  const sc = screenColors(p);
  let body: React.JSX.Element;
  let label: string;
  switch (s.phase) {
    case "store":
      body = StoreScreen(s, sc);
      label = "The Oxen Trail: outfit your wagon at the general store";
      break;
    case "hunt":
      body = HuntScreen(s, sc);
      label = "The Oxen Trail: hunting — type the word quickly";
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

/** Attract screen: a scenic title card — wagon on the plains under mountains. */
function OregonAttract(p: ThemePalette) {
  const c = sceneColors(p);
  const hz = 20;
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
            {/* oxen */}
            <Px x={x + 13} y={y + 2} w={5} h={3} fill={c.ox} />
            <Px x={x + 12} y={y + 2} w={1} h={2} fill={c.ox} />
          </g>
        );
      })()}
      <PxText x={(COLS * U) / 2} y={7 * U} size={15} fill={c.accent} shadow="#000" anchor="middle">THE OXEN TRAIL</PxText>
      <PxText x={(COLS * U) / 2} y={11 * U} size={7} fill={c.text} shadow="#000" anchor="middle">Guide your wagon to Oregon</PxText>
    </GameFrame>
  );
}

export const OxenTrailGame: HeroGameDefinition<OregonState> = {
  title: "The Oxen Trail",
  tab: "Trail",
  initialState: freshGame,
  onStart: () => freshGame(),
  handleKey: oregonKey,
  update: oregonUpdate,
  render: OregonRender,
  renderAttract: OregonAttract,
  keys: (key) => key.length === 1 || ["Enter", "Backspace", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(key),
};
