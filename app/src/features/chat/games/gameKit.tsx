// Shared building blocks for the empty-state hero games. Every game paints onto
// the same COLS×ROWS logical grid — each cell U viewBox units square — so the
// art snaps to a pixel lattice and reads as genuine 8-bit work. Games pull their
// colors from the active theme palette, so each one re-skins per theme.

import type { ThemePalette } from "../../../lib/types";

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

// Empty-state hero games use a small definition object: state creation, input,
// frame updates, and rendering are all swappable. To add a new game, implement
// `HeroGameDefinition` and register it in HERO_GAMES (see heroGames.tsx).
//
// Games don't start on their own. The hero shows the game's attract screen (a
// static title card) until the player enters the ↑ ↑ ↓ ↓ start combo — so arrow
// keys stay free for normal use and the empty state is calm by default.
export interface HeroGameDefinition<State = unknown> {
  title: string;
  /** One-word label for the cabinet-select tabs. Falls back to `title`. */
  tab?: string;
  initialState: () => State;
  /** Fresh run when the player starts from the attract screen. Receives the
      previous state so a game can carry things like a high score across. */
  onStart?: (state: State) => State;
  handleKey: (state: State, key: string) => State;
  update: (state: State, dt: number) => State;
  render: (state: State, p: ThemePalette) => React.JSX.Element;
  /** Static title card shown until the start combo is entered. */
  renderAttract?: (p: ThemePalette) => React.JSX.Element;
  /** Which keys, while playing, route to handleKey. Defaults to the arrow keys
      plus space/enter. Text games widen this to digits and letters. */
  keys?: (key: string) => boolean;
  /** Optional control hint the wrapper overlays at the bottom of the screen
      while playing. Omit it if the game draws its own per-screen footers (else
      the two collide). */
  help?: string;
}
