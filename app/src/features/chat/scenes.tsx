// Pixel hero scenes — crisp SVG painted entirely from the active theme's palette
// so each one re-skins per theme. Scenes are addressed by name from a theme's
// `style.scene`, and registered in SCENES below.
//
// ADDING A SCENE: write a `function MyScene({ p }: SceneProps)` that returns an
// <svg> (reuse Px/poly and the COLS×ROWS grid), then add one line to SCENES.
// A theme then opts in with `scene = "my-scene"` in its `[style]` block. No
// other wiring is needed.

import type { ThemePalette } from "../../lib/types";

export interface SceneProps {
  p: ThemePalette;
}

const U = 4; // pixel size in viewBox units
const COLS = 72;
const ROWS = 34;
const HZ = 22; // horizon row

/** One snapped pixel block. */
function Px({ x, y, w, h, fill, o }: { x: number; y: number; w: number; h: number; fill: string; o?: number }) {
  return <rect x={x * U} y={y * U} width={w * U} height={h * U} fill={fill} opacity={o} />;
}

function poly(pts: [number, number][], fill: string, o?: number) {
  return <polygon points={pts.map(([x, y]) => `${x * U},${y * U}`).join(" ")} fill={fill} opacity={o} />;
}

function Frame({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <svg
      className="trail-scene pixelated"
      viewBox={`0 0 ${COLS * U} ${ROWS * U}`}
      preserveAspectRatio="xMidYMid meet"
      role="img"
      aria-label={label}
    >
      {children}
    </svg>
  );
}

/** The Oregon Trail wagon: mountains, a pixel sun, prairie, and a covered wagon. */
export function TrailScene({ p }: SceneProps) {
  const sky = p?.background || "#0f1115";
  const mountain = p?.secondary || "#aa6e3c";
  const mountainBack = p?.muted || "#968d7d";
  const snow = p?.text || "#ece2ce";
  const sun = p?.title || "#f0be8c";
  const grass = p?.primary || "#60b060";
  const trail = p?.title || "#f0be8c";
  const wood = p?.secondary || "#aa6e3c";
  const canvas = p?.text || "#ece2ce";
  const ox = p?.text || "#ece2ce";
  const dark = sky;

  const wx0 = 43;
  const wx1 = 57;
  const domeBase = 23;
  const domeAmp = 7;
  const bars = [];
  for (let i = wx0; i <= wx1; i++) {
    const f = (i - wx0) / (wx1 - wx0);
    const top = domeBase - Math.round(domeAmp * Math.sin(Math.PI * f));
    bars.push(<Px key={i} x={i} y={top} w={1} h={domeBase - top} fill={canvas} />);
  }

  return (
    <Frame label="A covered wagon on the prairie beneath mountains">
      <Px x={0} y={0} w={COLS} h={ROWS} fill={sky} />
      <Px x={58} y={3} w={6} h={6} fill={sun} />
      <Px x={57} y={5} w={1} h={2} fill={sun} o={0.7} />
      <Px x={64} y={5} w={1} h={2} fill={sun} o={0.7} />
      <Px x={60} y={2} w={2} h={1} fill={sun} o={0.7} />
      <Px x={60} y={9} w={2} h={1} fill={sun} o={0.7} />
      {poly([[0, HZ], [9, 11], [18, 17], [28, 7], [38, 16], [48, 9], [58, 17], [66, 11], [72, 18], [72, HZ]], mountainBack, 0.65)}
      {poly([[0, HZ], [12, 15], [22, 20], [34, 12], [44, 19], [54, 14], [66, 20], [72, 17], [72, HZ]], mountain)}
      {poly([[31, 15], [34, 12], [37, 15]], snow)}
      {poly([[51, 17], [54, 14], [57, 17]], snow)}
      <Px x={0} y={HZ} w={COLS} h={ROWS - HZ} fill={grass} />
      {poly([[30, HZ], [42, HZ], [38, ROWS], [22, ROWS]], trail, 0.5)}
      {/* ox */}
      <Px x={34} y={24} w={7} h={4} fill={ox} />
      <Px x={31} y={24} w={3} h={3} fill={ox} />
      <Px x={31} y={23} w={1} h={1} fill={mountainBack} />
      <Px x={33} y={23} w={1} h={1} fill={mountainBack} />
      <Px x={35} y={28} w={1} h={2} fill={mountainBack} />
      <Px x={37} y={28} w={1} h={2} fill={mountainBack} />
      <Px x={39} y={28} w={1} h={2} fill={mountainBack} />
      <Px x={41} y={25} w={2} h={1} fill={wood} />
      {/* wagon */}
      {bars}
      <Px x={wx0} y={domeBase} w={wx1 - wx0 + 1} h={4} fill={wood} />
      <Px x={45} y={27} w={4} h={4} fill={canvas} />
      <Px x={46} y={28} w={2} h={2} fill={dark} />
      <Px x={53} y={27} w={4} h={4} fill={canvas} />
      <Px x={54} y={28} w={2} h={2} fill={dark} />
    </Frame>
  );
}

/** A synthwave outrun grid: a banded neon sun over a perspective grid floor. */
export function GridScene({ p }: SceneProps) {
  const sky = p?.background || "#140926";
  const sunTop = p?.title || "#ff71ce";
  const sunMid = p?.secondary || "#b967ff";
  const sunLow = p?.primary || "#01cdfe";
  const grid = p?.primary || "#01cdfe";
  const ridge = p?.secondary || "#b967ff";

  const cx = 36;
  const cy = 12;
  const R = 9;

  // Pixel disc, banded top→bottom (pink → purple → cyan).
  const sun = [];
  for (let dy = -R; dy <= R; dy++) {
    const y = cy + dy;
    if (y >= HZ) break; // never below the horizon
    const w = Math.round(Math.sqrt(Math.max(0, R * R - dy * dy)));
    if (w <= 0) continue;
    const color = dy < -R / 3 ? sunTop : dy < R / 3 ? sunMid : sunLow;
    sun.push(<Px key={`s${dy}`} x={cx - w} y={y} w={w * 2} h={1} fill={color} />);
  }
  // Horizontal slits across the lower half — the signature striped sun.
  const slits = [];
  for (let i = 0; i < 4; i++) {
    const y = cy + 2 + i * 2;
    if (y >= HZ) break;
    slits.push(<Px key={`sl${i}`} x={cx - R} y={y} w={R * 2} h={1} fill={sky} />);
  }

  // Perspective floor: horizontal lines bunching toward the horizon, vertical
  // lines converging on the vanishing point at (cx, HZ).
  const floorH = [];
  const N = 7;
  for (let k = 1; k <= N; k++) {
    const y = HZ + Math.round((ROWS - HZ) * (k / N) ** 1.9);
    floorH.push(
      <line key={`h${k}`} x1={0} y1={y * U} x2={COLS * U} y2={y * U} stroke={grid} strokeWidth={1} opacity={0.75} />,
    );
  }
  const floorV = [];
  for (let vx = -6; vx <= COLS + 6; vx += 9) {
    floorV.push(
      <line key={`v${vx}`} x1={vx * U} y1={ROWS * U} x2={cx * U} y2={HZ * U} stroke={grid} strokeWidth={1} opacity={0.65} />,
    );
  }

  return (
    <Frame label="A neon sun setting over a synthwave grid">
      <Px x={0} y={0} w={COLS} h={ROWS} fill={sky} />
      {/* faint upper-sky glow bands */}
      <Px x={0} y={0} w={COLS} h={6} fill={sunTop} o={0.06} />
      <Px x={0} y={6} w={COLS} h={6} fill={ridge} o={0.05} />
      {sun}
      {slits}
      {/* distant ridge along the horizon */}
      {poly([[0, HZ], [10, HZ - 2], [20, HZ - 1], [30, HZ - 3], [42, HZ - 1], [54, HZ - 3], [64, HZ - 1], [72, HZ - 2], [72, HZ]], ridge, 0.8)}
      <Px x={0} y={HZ} w={COLS} h={ROWS - HZ} fill={sky} />
      {/* glowing horizon line */}
      <Px x={0} y={HZ} w={COLS} h={1} fill={grid} />
      {floorH}
      {floorV}
    </Frame>
  );
}

/** Scene registry. Add a `name → component` entry to make a scene selectable. */
export const SCENES: Record<string, (props: SceneProps) => React.JSX.Element> = {
  trail: TrailScene,
  grid: GridScene,
};

/** Resolve a scene by name, falling back to the trail. `"none"` renders nothing. */
export function getScene(name: string | undefined): ((props: SceneProps) => React.JSX.Element) | null {
  if (name === "none") return null;
  return (name && SCENES[name]) || TrailScene;
}
