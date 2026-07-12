// Tumbleweed Dodge — an 8-bit endless runner. Steer the ox down a converging
// Oregon-Trail vista, dodging tumbleweeds that roll down toward you. The attract
// screen doubles as a title card built from the old hero's trail tableau.

import type { ThemePalette } from "../../../lib/types";
import { clamp, COLS, GameFrame, Px, poly, PxText, ROWS, sceneColors, SceneColors, U, type HeroGameDefinition } from "./gameKit";

interface RunnerState {
  oxLane: number;
  oxRow: number;
  weeds: Tumbleweed[];
  score: number;
  highScore: number;
  nextWeedIn: number;
  /** Distance rolled so far — drives the ground scroll and sprite animation. */
  dist: number;
  running: boolean;
  crashed: boolean;
}

interface Tumbleweed {
  id: number;
  lane: number;
  row: number;
  speed: number;
  spin: number;
}

const HZ = 11; // horizon row: sky + mountains above, prairie below

// The trail converges toward the horizon; a lane's x depends on its row.
const LANES_BOTTOM = [21, 36, 51];
const LANES_HORIZON = [31.5, 36, 40.5];
const RUTS_BOTTOM = [28.5, 43.5];
const RUTS_HORIZON = [33.8, 38.2];

let nextWeedId = 1;

function laneX(lane: number, row: number) {
  const t = clamp((row - HZ) / (ROWS - HZ), 0, 1);
  return LANES_HORIZON[lane] + (LANES_BOTTOM[lane] - LANES_HORIZON[lane]) * t;
}

function resetRunner(highScore = 0): RunnerState {
  return {
    oxLane: 1,
    oxRow: 27,
    weeds: [],
    score: 0,
    highScore,
    nextWeedIn: 0.7,
    dist: 0,
    running: true,
    crashed: false,
  };
}

function runnerKey(state: RunnerState, key: string): RunnerState {
  if (state.crashed) {
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " ", "Enter"].includes(key)) {
      return resetRunner(state.highScore);
    }
    return state;
  }

  if (key === "ArrowLeft") return { ...state, oxLane: clamp(state.oxLane - 1, 0, LANES_BOTTOM.length - 1) };
  if (key === "ArrowRight") return { ...state, oxLane: clamp(state.oxLane + 1, 0, LANES_BOTTOM.length - 1) };
  if (key === "ArrowUp") return { ...state, oxRow: clamp(state.oxRow - 2, 23, 29) };
  if (key === "ArrowDown") return { ...state, oxRow: clamp(state.oxRow + 2, 23, 29) };
  return state;
}

function runnerUpdate(state: RunnerState, dt: number): RunnerState {
  if (!state.running || state.crashed) return state;

  const score = state.score + dt * 12;
  const difficulty = Math.min(1.8, 1 + score / 450);
  const dist = state.dist + dt * 12 * difficulty;
  let nextWeedIn = state.nextWeedIn - dt;
  let weeds = state.weeds.map((weed) => ({
    ...weed,
    row: weed.row + weed.speed * difficulty * dt,
    spin: weed.spin + dt * 5,
  }));

  if (nextWeedIn <= 0) {
    const occupied = new Set(weeds.filter((w) => w.row < HZ + 6).map((w) => w.lane));
    const openLanes = LANES_BOTTOM.map((_, lane) => lane).filter((lane) => !occupied.has(lane));
    const lane = openLanes.length ? openLanes[Math.floor(Math.random() * openLanes.length)] : Math.floor(Math.random() * LANES_BOTTOM.length);
    weeds = [...weeds, { id: nextWeedId++, lane, row: HZ - 3, speed: 11 + Math.random() * 4, spin: Math.random() * Math.PI * 2 }];
    nextWeedIn = Math.max(0.38, 1.05 - score / 350) + Math.random() * 0.45;
  }

  weeds = weeds.filter((weed) => weed.row < ROWS + 4);
  const crashed = weeds.some((weed) => weed.lane === state.oxLane && Math.abs(weed.row - state.oxRow) < 3.2);
  const highScore = Math.max(state.highScore, Math.floor(score));

  return { ...state, weeds, score, highScore, nextWeedIn, dist, crashed, running: !crashed };
}

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
      <Px x={0} y={0} w={COLS} h={ROWS} fill={c.sky} />
      {/* pixel sun with rays */}
      <Px x={57} y={2} w={5} h={5} fill={c.sun} />
      <Px x={56} y={4} w={1} h={1} fill={c.sun} o={0.7} />
      <Px x={62} y={4} w={1} h={1} fill={c.sun} o={0.7} />
      <Px x={59} y={1} w={1} h={1} fill={c.sun} o={0.7} />
      <Px x={59} y={7} w={1} h={1} fill={c.sun} o={0.7} />
      {/* mountain ranges along the horizon */}
      {poly([[0, HZ], [8, 5], [16, 9], [26, 3], [36, 8], [47, 4], [57, 9], [65, 6], [72, 9], [72, HZ]], c.mountainBack, 0.6)}
      {poly([[0, HZ], [10, 7], [20, HZ - 1], [30, 6], [40, HZ - 1], [50, 7], [62, HZ - 1], [72, 8], [72, HZ]], c.mountain)}
      {poly([[28, 8], [30, 6], [32, 8]], c.snow)}
      {poly([[48, 9], [50, 7], [52, 9]], c.snow)}
      {/* prairie and the trail converging on the pass */}
      <Px x={0} y={HZ} w={COLS} h={ROWS - HZ} fill={c.grass} />
      {poly([[29, HZ], [43, HZ], [59, ROWS], [13, ROWS]], c.trail, 0.5)}
      {ruts}
      {tufts}
    </g>
  );
}

/** The ox, with a two-frame gallop. `step` toggles which legs are planted. */
function OxSprite({ x, y, step, fill, shadow }: { x: number; y: number; step: number; fill: string; shadow: string }) {
  const px = Math.round(x);
  const py = Math.round(y);
  return (
    <g transform={`translate(${(px - 5) * U} ${(py - 3) * U})`}>
      <Px x={1} y={3} w={9} h={4} fill={shadow} o={0.3} />
      {/* horns — realistic curve: light base, dark keratin tips curling up and out */}
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
      <Px x={10} y={2} w={1} h={1} fill={fill} />
      {/* galloping legs */}
      {step === 0 ? (
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

function WeedSprite({ x, y, spin, small, fill, dark }: { x: number; y: number; spin: number; small: boolean; fill: string; dark: string }) {
  const px = Math.round(x);
  const py = Math.round(y);
  const frame = Math.floor(spin * 2) % 2;
  const ring = small ? WEED_RING_SMALL : WEED_RING_BIG;
  const spokes = small ? [] : frame === 0 ? WEED_SPOKES_X : WEED_SPOKES_PLUS;
  return (
    <g>
      {ring.map(([dx, dy]) => (
        <Px key={`${dx},${dy}`} x={px + dx} y={py + dy} w={1} h={1} fill={fill} />
      ))}
      {spokes.map(([dx, dy]) => (
        <Px key={`s${dx},${dy}`} x={px + dx} y={py + dy} w={1} h={1} fill={fill} o={0.75} />
      ))}
      <Px x={px} y={py} w={1} h={1} fill={dark} o={0.6} />
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

function RunnerRender(state: RunnerState, p: ThemePalette) {
  const c = sceneColors(p);
  const oxStep = Math.floor(state.dist * 0.6) % 2;

  return (
    <GameFrame label="Tumbleweed Dodge: steer the ox around tumbleweeds rolling down the trail">
      <Prairie c={c} dist={state.dist} />
      {state.weeds.map((weed) =>
        weed.row < HZ ? null : (
          <WeedSprite
            key={weed.id}
            x={laneX(weed.lane, weed.row)}
            y={weed.row}
            spin={weed.spin}
            small={weed.row < HZ + 8}
            fill={c.weed}
            dark={c.sky}
          />
        ),
      )}
      <OxSprite x={laneX(state.oxLane, state.oxRow)} y={state.oxRow} step={state.crashed ? 0 : oxStep} fill={c.ox} shadow={c.line} />
      <PxText x={8} y={17} size={11} fill={c.text} shadow={c.sky}>
        {Math.floor(state.score).toString().padStart(4, "0")}
      </PxText>
      <PxText x={COLS * U - 8} y={(ROWS - 2) * U} size={9} fill={c.text} shadow={c.sky} anchor="end">
        {`HI ${state.highScore.toString().padStart(4, "0")}`}
      </PxText>
      {state.crashed && (
        <g>
          <Px x={13} y={9} w={46} h={15} fill={c.accent} />
          <Px x={14} y={10} w={44} h={13} fill="#000" />
          <PxText x={(COLS * U) / 2} y={14.5 * U} size={13} fill={c.accent} shadow="#000" anchor="middle">
            TUMBLED!
          </PxText>
          <PxText x={(COLS * U) / 2} y={18 * U} size={8} fill={c.text} shadow="#000" anchor="middle">
            {`SCORE ${Math.floor(state.score).toString().padStart(4, "0")}`}
          </PxText>
          <PxText x={(COLS * U) / 2} y={21 * U} size={8} fill={c.text} shadow="#000" anchor="middle">
            press an arrow to ride again
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
  return (
    <GameFrame label="Tumbleweed Dodge title screen: a covered wagon on the trail beneath mountains">
      <Prairie c={c} dist={0} />
      <WagonSprite x={39} y={15} canvas={c.snow} wood={c.mountain} dark={c.sky} />
      <OxSprite x={31} y={24} step={0} fill={c.ox} shadow={c.line} />
      <PxText x={(COLS * U) / 2} y={7 * U} size={15} fill={c.accent} shadow="#000" anchor="middle">
        TUMBLEWEED DODGE
      </PxText>
    </GameFrame>
  );
}

export const TumbleweedDodgeGame: HeroGameDefinition<RunnerState> = {
  title: "Tumbleweed Dodge",
  tab: "Dodge",
  initialState: () => resetRunner(),
  onStart: (state) => resetRunner(state.highScore),
  handleKey: runnerKey,
  update: runnerUpdate,
  render: RunnerRender,
  renderAttract: RunnerAttract,
  help: "← ↑ ↓ → dodge · esc makes camp",
};
