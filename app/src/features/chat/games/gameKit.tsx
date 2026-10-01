// Shared building blocks for the empty-state hero games. Every game paints onto
// the same COLS×ROWS logical grid — each cell U viewBox units square — so the
// art snaps to a pixel lattice and reads as genuine 8-bit work. Games pull their
// colors from the active theme palette, so each one re-skins per theme.

import type { ThemePalette } from "../../../lib/types";
import type { SfxName } from "./sfx";

export const COLS = 72;
export const ROWS = 34;
export const U = 4; // pixel size in viewBox units

export function clamp(n: number, min: number, max: number) {
  return Math.max(min, Math.min(max, n));
}

/** One snapped pixel block. */
export function Px({ x, y, w, h, fill, o }: { x: number; y: number; w: number; h: number; fill: string; o?: number }) {
  return <rect x={x * U} y={y * U} width={w * U} height={h * U} fill={fill} opacity={o} />;
}

export function poly(pts: [number, number][], fill: string, o?: number, key?: string) {
  return <polygon key={key} points={pts.map(([x, y]) => `${x * U},${y * U}`).join(" ")} fill={fill} opacity={o} />;
}

/** Pixel text with the stamped hard shadow the wordmark uses. Coordinates are in
    viewBox units (not grid cells), so callers place it precisely. */
export function PxText({
  x,
  y,
  size,
  fill,
  shadow,
  anchor,
  children,
}: {
  x: number;
  y: number;
  size: number;
  fill: string;
  shadow: string;
  anchor?: "start" | "middle" | "end";
  children: string;
}) {
  return (
    <g fontFamily="var(--font-readout)" fontSize={size} textAnchor={anchor}>
      <text x={x + 1.5} y={y + 1.5} fill={shadow}>
        {children}
      </text>
      <text x={x} y={y} fill={fill}>
        {children}
      </text>
    </g>
  );
}

/** The SVG "screen" every game renders into, sized to the shared grid. */
export function GameFrame({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <svg
      className="hero-game-canvas pixelated"
      viewBox={`0 0 ${COLS * U} ${ROWS * U}`}
      preserveAspectRatio="xMidYMid meet"
      role="img"
      aria-label={label}
    >
      {children}
    </svg>
  );
}

/** Resolve a game's working colors from a theme palette once, up front. */
export function sceneColors(p: ThemePalette) {
  return {
    sky: p?.background || "#0f1115",
    sun: p?.title || "#f0be8c",
    mountain: p?.secondary || "#aa6e3c",
    mountainBack: p?.muted || "#968d7d",
    snow: p?.text || "#ece2ce",
    grass: p?.primary || "#60b060",
    trail: p?.title || "#f0be8c",
    line: p?.border || p?.muted || "#968d7d",
    ox: p?.text || "#ece2ce",
    weed: p?.secondary || "#aa6e3c",
    text: p?.text || "#ece2ce",
    accent: p?.title || "#f0be8c",
    danger: p?.danger || "#c94c4c",
  };
}

export type SceneColors = ReturnType<typeof sceneColors>;

// ---- personal bests ---------------------------------------------------------
// Each cabinet keeps its own high score in localStorage so "beat your best"
// survives closing the app. Storage can be missing or throw (private mode,
// jsdom); every access is guarded and a miss simply reads as 0.

const BEST_PREFIX = "oxen-hero-best-";

export function loadBest(game: string): number {
  try {
    const raw = window.localStorage?.getItem(BEST_PREFIX + game);
    const n = raw == null ? 0 : Number(raw);
    return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
  } catch {
    return 0;
  }
}

/** Persist `score` if it beats the stored best; returns the new best. */
export function saveBest(game: string, score: number): number {
  const best = Math.max(loadBest(game), Math.floor(score));
  try {
    window.localStorage?.setItem(BEST_PREFIX + game, String(best));
  } catch {
    // Storage is a convenience only.
  }
  return best;
}

// ---- seeded randomness ----------------------------------------------------------
// Every game draws its randomness from one seedable generator (mulberry32) so
// tests can pin a sequence and "daily" runs share a layout: seed the generator
// with today's date and two players see the same trail. Games call `rand()`
// wherever they'd have called Math.random().

let rngState = (Date.now() ^ 0x9e3779b9) >>> 0;

export function seedRng(seed: number) {
  rngState = (seed >>> 0) || 0x1234567;
}

export function rand(): number {
  rngState = (rngState + 0x6d2b79f5) >>> 0;
  let t = rngState;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

export const randInt = (a: number, b: number) => a + Math.floor(rand() * (b - a + 1));
export const pickRand = <T,>(arr: T[]): T => arr[Math.floor(rand() * arr.length)];

/** A seed shared by everyone playing today (local date), plus a per-game salt. */
export function dailySeed(salt = 0): number {
  const d = new Date();
  const day = d.getFullYear() * 10000 + (d.getMonth() + 1) * 100 + d.getDate();
  return (Math.imul(day, 2654435761) ^ salt) >>> 0;
}

export function dailyLabel(): string {
  const d = new Date();
  return `${["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"][d.getMonth()]} ${d.getDate()}`;
}

const DAILY_KEY = "oxen-hero-daily";

/** Daily mode: seed each run from the date so runs are comparable. */
export function dailyPreference(): boolean {
  try {
    return window.localStorage?.getItem(DAILY_KEY) === "1";
  } catch {
    return false;
  }
}

export function setDailyPreference(on: boolean) {
  try {
    window.localStorage?.setItem(DAILY_KEY, on ? "1" : "0");
  } catch {
    // preference only
  }
}

/** Seed a fresh run: today's seed in daily mode, otherwise a fresh random one. */
export function seedRun(salt: number, daily = dailyPreference()) {
  seedRng(daily ? dailySeed(salt) : (Math.random() * 4294967296) >>> 0);
}

// ---- small persisted preferences ------------------------------------------------

export function loadPref(key: string): string | null {
  try {
    return window.localStorage?.getItem("oxen-hero-" + key) ?? null;
  } catch {
    return null;
  }
}

export function savePref(key: string, value: string) {
  try {
    window.localStorage?.setItem("oxen-hero-" + key, value);
  } catch {
    // preference only
  }
}

// ---- sound cues --------------------------------------------------------------------
// Games are pure state, so they can't play audio. Instead they append named cues
// to `state.sfx` with rising ids; the wrapper plays anything newer than the last
// id it saw. The queue is capped so a long-lived state never grows.

export interface SfxEvent {
  id: number;
  name: SfxName;
}

let nextSfxId = 1;

export function pushSfx(queue: SfxEvent[] | undefined, ...names: SfxName[]): SfxEvent[] {
  const out = [...(queue ?? [])];
  for (const name of names) out.push({ id: nextSfxId++, name });
  return out.length > 12 ? out.slice(out.length - 12) : out;
}

// ---- host requests ------------------------------------------------------------------
// Games are pure state, so they can't call the backend either. A game that
// needs data (the study cabinet asks for questions and grades) queues requests
// the same way it queues sound: append to `state.requests` with rising ids, and
// the wrapper performs anything newer than the last id it saw through the
// injected `HeroGameHost`, handing the outcome back via the definition's
// `deliver`. Tests drive a game by calling `deliver` directly, or hand the
// wrapper a fake host.

export interface GameRequest {
  id: number;
  /** What to do, e.g. `"study.batch"`. The host decides what kinds it serves. */
  kind: string;
  payload: unknown;
}

export type GameResult = { ok: true; value: unknown } | { ok: false; error: string };

/** The backend a cabinet may call. One method, so a fake is one function. */
export interface HeroGameHost {
  perform: (kind: string, payload: unknown) => Promise<unknown>;
}

let nextRequestId = 1;

export function pushRequest(queue: GameRequest[] | undefined, kind: string, payload: unknown = null): GameRequest[] {
  const out = [...(queue ?? []), { id: nextRequestId++, kind, payload }];
  return out.length > 16 ? out.slice(out.length - 16) : out;
}

/** The newest request id in a queue (0 when empty) — what a game stores to
    recognise the reply it is waiting for. */
export function lastRequestId(queue: GameRequest[] | undefined): number {
  return queue && queue.length ? queue[queue.length - 1].id : 0;
}

/** A line of typed input a game is waiting for (see `textEntry`). */
export interface TextEntry {
  /** Accessible label for the input. */
  label: string;
  placeholder?: string;
}

// ---- reduced motion -----------------------------------------------------------------
// Shake, hit-stop and flashes scale down for players who asked the OS for less
// motion. Games multiply their juice durations/amplitudes by `motionScale()`.

let motionQuery: MediaQueryList | null | undefined;

export function motionScale(): number {
  if (motionQuery === undefined) {
    try {
      motionQuery = typeof window !== "undefined" && window.matchMedia ? window.matchMedia("(prefers-reduced-motion: reduce)") : null;
    } catch {
      motionQuery = null;
    }
  }
  return motionQuery?.matches ? 0.25 : 1;
}

// ---- tiny deterministic helpers shared by the arcade games -----------------

/** A screen-shake offset for `t` seconds after a hit: fast decaying jitter. */
export function shakeOffset(t: number, amp = 3): [number, number] {
  if (t <= 0) return [0, 0];
  const a = amp * motionScale() * Math.min(1, t * 4);
  return [Math.sin(t * 90) * a, Math.cos(t * 70) * a * 0.6];
}

/** Style helper: a full-frame group translated by a shake offset (viewBox units). */
export function shakeTransform(t: number, amp = 3) {
  const [dx, dy] = shakeOffset(t, amp);
  return dx || dy ? `translate(${dx.toFixed(2)} ${dy.toFixed(2)})` : undefined;
}

/** Per-frame particle used for dust, sparks, and weed shrapnel. */
export interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number; // seconds remaining
  max: number; // starting life, for fade
  size: number; // in grid cells
  kind: "dust" | "spark" | "shard";
}

export function stepParticles(ps: Particle[], dt: number, gravity = 0): Particle[] {
  const out: Particle[] = [];
  for (const p of ps) {
    const life = p.life - dt;
    if (life <= 0) continue;
    out.push({ ...p, x: p.x + p.vx * dt, y: p.y + p.vy * dt, vy: p.vy + gravity * dt, life });
  }
  return out;
}

export function burst(x: number, y: number, n: number, kind: Particle["kind"], speed: number, life: number, size = 1): Particle[] {
  const ps: Particle[] = [];
  for (let i = 0; i < n; i++) {
    const a = rand() * Math.PI * 2;
    const v = speed * (0.4 + rand() * 0.6);
    ps.push({ x, y, vx: Math.cos(a) * v, vy: Math.sin(a) * v - speed * 0.3, life: life * (0.6 + rand() * 0.4), max: life, size, kind });
  }
  return ps;
}

export function Particles({ ps, fill, spark }: { ps: Particle[]; fill: string; spark: string }) {
  return (
    <g>
      {ps.map((p, i) => (
        <rect
          key={i}
          x={Math.round(p.x) * U}
          y={Math.round(p.y) * U}
          width={p.size * U}
          height={p.size * U}
          fill={p.kind === "spark" ? spark : fill}
          opacity={Math.max(0, Math.min(1, p.life / p.max)) * (p.kind === "dust" ? 0.6 : 1)}
        />
      ))}
    </g>
  );
}

/** A floating score popup ("+50 NEAR MISS") that drifts up and fades. */
export interface Popup {
  x: number; // viewBox units
  y: number;
  text: string;
  life: number;
  max: number;
  big?: boolean;
}

export function stepPopups(ps: Popup[], dt: number): Popup[] {
  return ps.map((p) => ({ ...p, y: p.y - dt * 14, life: p.life - dt })).filter((p) => p.life > 0);
}

export function Popups({ ps, fill, shadow }: { ps: Popup[]; fill: string; shadow: string }) {
  return (
    <g>
      {ps.map((p, i) => (
        <g key={i} opacity={Math.min(1, (p.life / p.max) * 2)}>
          <PxText x={p.x} y={p.y} size={p.big ? 11 : 8} fill={fill} shadow={shadow} anchor="middle">
            {p.text}
          </PxText>
        </g>
      ))}
    </g>
  );
}

// Empty-state hero games use a small definition object: state creation, input,
// frame updates, and rendering are all swappable. To add a new game, implement
// `HeroGameDefinition` and register it in HERO_GAMES (see heroGames.tsx).
//
// Games don't start on their own. The hero shows the game's attract screen (a
// static title card) until the player enters the ↑ ↑ ↓ ↓ start combo — so arrow
// keys stay free for normal use and the empty state is calm by default.
/** A tap or swipe on the playfield, in playfield fractions (0..1). */
export interface PointerInput {
  kind: "tap" | "swipe";
  x: number;
  y: number;
  /** Swipe direction as an arrow key name; taps carry none. */
  dir?: "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown";
}

/** Map a pointer gesture onto the keys a keyboard-only game already handles:
    taps on the left/right halves steer, swipes are the arrow they point. */
export function pointerAsKey(p: PointerInput): string {
  if (p.kind === "swipe" && p.dir) return p.dir;
  return p.x < 0.5 ? "ArrowLeft" : "ArrowRight";
}

export interface HeroGameDefinition<State = unknown> {
  title: string;
  /** One-word label for the cabinet-select tabs. Falls back to `title`. */
  tab?: string;
  /** Salt mixed into the daily seed so cabinets don't share a sequence. */
  seedSalt?: number;
  initialState: () => State;
  /** Fresh run when the player starts from the attract screen. Receives the
      previous state so a game can carry things like a high score across. */
  onStart?: (state: State) => State;
  handleKey: (state: State, key: string) => State;
  /** Key releases, for games with hold-to-move controls. Same key filter as
      `handleKey`. */
  handleKeyUp?: (state: State, key: string) => State;
  /** Non-arrow keys pressed on the attract screen (arrows feed the start
      combo). Lets a game offer choices — a rider, a difficulty — before play. */
  handleAttractKey?: (state: State, key: string) => State;
  /** Taps and swipes on the playfield while playing. Defaults to
      `handleKey(pointerAsKey(p))` so keyboard games get touch for free. */
  handlePointer?: (state: State, p: PointerInput) => State;
  /** Called when the wrapper pauses (window blur) so a game can freeze
      hold-to-move input; the default just stops the frame loop. */
  onPause?: (state: State) => State;
  update: (state: State, dt: number) => State;
  render: (state: State, p: ThemePalette) => React.JSX.Element;
  /** Static title card shown until the start combo is entered. Receives the
      current state so a cabinet can show something it loaded (a profile). */
  renderAttract?: (p: ThemePalette, state: State) => React.JSX.Element;
  /** The outcome of a request the game queued on `state.requests`. Runs on
      the attract screen too, so a cabinet can load data before play. */
  deliver?: (state: State, request: GameRequest, result: GameResult) => State;
  /** When this returns an entry, the wrapper shows a text input under the
      screen and routes the submitted line to `handleText` — for answers
      that need typing, which the key handler can't collect. */
  textEntry?: (state: State) => TextEntry | null;
  handleText?: (state: State, text: string) => State;
  /** Hide the DAILY switch: the game has no seeded run to share. */
  noDaily?: boolean;
  /** Which keys, while playing, route to handleKey. Defaults to the arrow keys
      plus space/enter. Text games widen this to digits and letters. */
  keys?: (key: string) => boolean;
  /** Optional control hint the wrapper overlays at the bottom of the screen
      while playing. Omit it if the game draws its own per-screen footers (else
      the two collide). */
  help?: string;
}
