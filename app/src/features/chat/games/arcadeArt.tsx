import { COLS, Px, PxText, ROWS, U, type SceneColors } from "./gameKit";

/** Deterministic scenery never consumes the gameplay random stream. */
export function Vista({ c, horizon = 17, travel = 0, night = false }: { c: SceneColors; horizon?: number; travel?: number; night?: boolean }) {
  const drift = Math.floor(travel / 30) % COLS;
  return (
    <g shapeRendering="crispEdges" aria-hidden="true">
      <Px x={0} y={0} w={COLS} h={ROWS} fill={c.sky} />
      {[0, 1, 2, 3].map((i) => <Px key={i} x={0} y={horizon - 8 + i * 2} w={COLS} h={2} fill={c.sun} o={0.025 + i * 0.035} />)}
      {Array.from({ length: 20 }, (_, i) => <Px key={`star${i}`} x={(i * 19 + 3) % COLS} y={(i * 7) % Math.max(1, horizon - 5)} w={0.5} h={0.5} fill={c.snow} o={night ? 0.5 + (i % 3) * 0.2 : 0.12} />)}
      {[4, 6, 8, 8, 6, 4].map((w, i) => <Px key={`sun${i}`} x={55 + (8 - w) / 2} y={horizon - 11 + i} w={w} h={1} fill={c.sun} o={night ? 0.65 : 1 - i * 0.08} />)}
      {[0, 1, 2].map((i) => (
        <g key={`cloud${i}`} opacity={0.1}>
          <Px x={(i * 29 + 7 - drift + COLS) % COLS} y={horizon - 9 + i * 2} w={9} h={0.5} fill={c.snow} />
          <Px x={(i * 29 + 9 - drift + COLS) % COLS} y={horizon - 10 + i * 2} w={5} h={1} fill={c.snow} />
        </g>
      ))}
      {[0, 1, 2].map((layer) => (
        <g key={layer}>
          {Array.from({ length: COLS }, (_, x) => {
            const wx = x + Math.floor(travel / (70 - layer * 18));
            const ridge = Math.round(2 + Math.abs(Math.sin(wx * 0.095 + layer * 2)) * (layer === 0 ? 8 : 4) + Math.sin(wx * 0.33) * 1.2);
            const y = horizon - ridge + layer;
            return <g key={x}>
              <Px x={x} y={y} w={1} h={horizon - y + 1} fill={layer === 0 ? c.mountainBack : layer === 1 ? c.mountain : c.grass} o={layer === 0 ? 0.45 : layer === 1 ? 0.7 : 1} />
              {layer === 0 && ridge > 8 && <Px x={x} y={y} w={1} h={1} fill={c.snow} o={0.6} />}
              {layer === 2 && x % 3 === 0 && <Px x={x} y={y} w={1} h={horizon - y + 1} fill={c.sky} o={0.22} />}
            </g>;
          })}
        </g>
      ))}
      <Px x={0} y={horizon} w={COLS} h={ROWS - horizon} fill={c.grass} />
      <Px x={0} y={horizon} w={COLS} h={ROWS - horizon} fill={c.sky} o={0.35} />
      {Array.from({ length: 70 }, (_, i) => <Px key={`grass${i}`} x={(i * 23) % COLS} y={horizon + ((i * 7 + Math.floor(travel)) % (ROWS - horizon))} w={i % 3 === 0 ? 2 : 0.5} h={0.5} fill={i % 4 === 0 ? c.sun : c.sky} o={i % 4 === 0 ? 0.25 : 0.2} />)}
      {[0, 1, 2].map((i) => <g key={`bird${i}`} fill={c.sky} opacity={0.7}>
        <Px x={10 + i * 3} y={horizon - 8 + (i % 2)} w={0.5} h={0.5} fill={c.snow} />
        <Px x={10.5 + i * 3} y={horizon - 7.5 + (i % 2)} w={1} h={0.5} fill={c.snow} />
        <Px x={11.5 + i * 3} y={horizon - 8 + (i % 2)} w={0.5} h={0.5} fill={c.snow} />
      </g>)}
    </g>
  );
}

export function Pine({ x, y, c, scale = 1 }: { x: number; y: number; c: SceneColors; scale?: number }) {
  return <g transform={`translate(${x * U} ${y * U}) scale(${scale})`}>
    <Px x={-2} y={0} w={6} h={1} fill={c.sky} o={0.4} />
    <Px x={0} y={-3} w={1} h={3} fill={c.mountain} />
    {[1, 3, 5, 3, 7].map((w, i) => <g key={i}>
      <Px x={-(w - 1) / 2} y={-7 + i} w={w} h={1} fill={c.grass} />
      <Px x={0} y={-7 + i} w={(w + 1) / 2} h={1} fill={c.sky} o={0.4} />
    </g>)}
    <Px x={-1} y={-5} w={1} h={0.5} fill={c.sun} o={0.6} />
  </g>;
}

export function Wagon({ x, y, c }: { x: number; y: number; c: SceneColors }) {
  return <g transform={`translate(${x * U} ${y * U})`}>
    <Px x={-2} y={8} w={21} h={1} fill={c.sky} o={0.4} />
    {[3, 1, 0, 0, 0, 1, 3].map((top, i) => <g key={i}>
      <Px x={i} y={top} w={1} h={5 - top} fill={i > 4 ? c.accent : c.snow} />
      {i % 2 === 0 && <Px x={i} y={top + 1} w={0.5} h={4 - top} fill={c.mountain} o={0.25} />}
    </g>)}
    <Px x={0} y={5} w={8} h={2} fill={c.mountain} />
    <Px x={1} y={5} w={6} h={0.5} fill={c.sun} />
    {[1, 6].map((x) => <g key={x}><Px x={x} y={6} w={2} h={2} fill={c.sky} /><Px x={x + 0.5} y={6.5} w={1} h={1} fill={c.accent} /></g>)}
    <Px x={8} y={6} w={4} h={0.5} fill={c.mountain} />
    <Px x={11} y={4} w={5} h={2} fill={c.snow} />
    <Px x={15} y={3} w={2} h={2} fill={c.snow} />
    <Px x={15} y={2} w={0.5} h={1} fill={c.accent} />
    <Px x={17} y={2} w={0.5} h={1} fill={c.accent} />
    <Px x={16.5} y={3.5} w={0.5} h={0.5} fill={c.sky} />
    {[11, 14, 16].map((x) => <Px key={x} x={x} y={6} w={0.5} h={2} fill={c.snow} />)}
  </g>;
}

export function TitlePlaque({ eyebrow, title, subtitle, c }: { eyebrow: string; title: string; subtitle: string; c: SceneColors }) {
  return <g>
    <rect x={24} y={11} width={240} height={49} fill={c.sky} opacity={0.91} />
    <rect x={24} y={11} width={240} height={1} fill={c.accent} opacity={0.7} />
    <rect x={24} y={59} width={240} height={1} fill={c.accent} opacity={0.4} />
    <PxText x={144} y={23} size={6} fill={c.accent} shadow={c.sky} anchor="middle">{eyebrow}</PxText>
    <PxText x={144} y={40} size={15} fill={c.snow} shadow={c.sky} anchor="middle">{title}</PxText>
    <PxText x={144} y={52} size={6.5} fill={c.accent} shadow={c.sky} anchor="middle">{subtitle}</PxText>
  </g>;
}

export function FieldTexture({ c, desert = false }: { c: SceneColors; desert?: boolean }) {
  return <g aria-hidden="true" shapeRendering="crispEdges">
    <Px x={0} y={3} w={COLS} h={ROWS - 6} fill={c.sky} o={0.3} />
    {Array.from({ length: 110 }, (_, i) => <g key={i}>
      <Px x={(i * 29 + 4) % COLS} y={4 + (i * i * 7 + i * 3) % (ROWS - 8)} w={i % 4 === 0 ? 2 : 0.5} h={0.5} fill={i % 5 === 0 ? c.sun : c.sky} o={0.12 + (i % 3) * 0.04} />
      {i % 7 === 0 && <Px x={(i * 29 + 4) % COLS} y={3.5 + (i * i * 7 + i * 3) % (ROWS - 8)} w={0.5} h={1} fill={desert ? c.accent : c.grass} o={0.5} />}
    </g>)}
    <Px x={0} y={3} w={COLS} h={0.5} fill={c.accent} o={0.3} />
    <Px x={0} y={ROWS - 3.5} w={COLS} h={0.5} fill={c.sky} o={0.5} />
  </g>;
}
