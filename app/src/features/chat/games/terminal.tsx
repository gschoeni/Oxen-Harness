// The green-screen terminal kit shared by the text cabinets (The Oxen Trail and
// the Trail of Understanding). Both games draw menus, message cards, a progress
// strip with landmarks, and an all-time score table onto the same COLS×ROWS
// screen; this file holds those pieces so the two games can't drift apart and a
// third text game is mostly content.

import type { ThemePalette } from "../../../lib/types";
import { COLS, loadPref, PxText, ROWS, savePref, sceneColors, U } from "./gameKit";

/** Screen size in viewBox units. */
export const W = COLS * U;
export const H = ROWS * U;

/** One line of readout text. Coordinates are viewBox units. */
export function Line({ x, y, c, size = 8, anchor, children }: { x: number; y: number; c: string; size?: number; anchor?: "start" | "middle" | "end"; children: string }) {
  return (
    <text x={x} y={y} fill={c} fontFamily="var(--font-readout)" fontSize={size} textAnchor={anchor} xmlSpace="preserve">
      {children}
    </text>
  );
}

/** The terminal's working colors, derived from the theme once per render. */
export function screenColors(p: ThemePalette) {
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

export type ScreenColors = ReturnType<typeof screenColors>;

/** The dimmed, outlined panel a message or choice card sits in. */
export function CardFrame({ y, height, tone }: { y: number; height: number; tone: string }) {
  return (
    <g>
      <rect x={16} y={y} width={W - 32} height={height} fill="#000" opacity={0.5} />
      <rect x={16} y={y} width={W - 32} height={height} fill="none" stroke={tone} strokeWidth={1} opacity={0.8} />
    </g>
  );
}

/** A centered card title in the stamped pixel face. */
export function CardTitle({ y, tone, sc, children, size = 11 }: { y: number; tone: string; sc: ScreenColors; children: string; size?: number }) {
  return (
    <PxText x={W / 2} y={y} size={size} fill={tone} shadow={sc.bg} anchor="middle">
      {children}
    </PxText>
  );
}

/** The dim "press any key" line every card ends with. */
export function Footer({ sc, children }: { sc: ScreenColors; children: string }) {
  return (
    <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">
      {children}
    </Line>
  );
}

/** A landmark tick on the progress strip; the shapes read as a fort, a river,
    a flag, or the journey's end. */
export type StripMarkType = "fort" | "river" | "flag" | "end";

export interface StripMark {
  /** Position along the strip, 0..1. */
  frac: number;
  type: StripMarkType;
  key: string;
}

/** The progress strip: a bar marked with every landmark ahead and behind, and
    a wagon inching toward the end. `frac` is how far along the traveler is. */
export function ProgressStrip({ frac, marks, sc, y = 20 }: { frac: number; marks: StripMark[]; sc: ScreenColors; y?: number }) {
  const barX = 10;
  const barW = W - 20;
  const wagonX = barX + barW * frac;
  return (
    <g>
      <rect x={barX} y={y} width={barW} height={4} fill={sc.frame} opacity={0.3} />
      <rect x={barX} y={y} width={barW * frac} height={4} fill={sc.good} opacity={0.65} />
      {marks.map((lm) => {
        const x = Math.round(barX + barW * lm.frac);
        const behind = frac >= lm.frac;
        const c = behind ? sc.dim : sc.accent;
        const o = behind ? 0.6 : 1;
        if (lm.type === "fort") return <rect key={lm.key} x={x - 1} y={y + 0.5} width={3} height={3} fill={c} opacity={o} />;
        if (lm.type === "river")
          return (
            <g key={lm.key} opacity={o}>
              <rect x={x - 1} y={y + 2} width={1} height={1} fill={c} />
              <rect x={x} y={y + 1} width={1} height={1} fill={c} />
              <rect x={x + 1} y={y + 2} width={1} height={1} fill={c} />
            </g>
          );
        if (lm.type === "end") return <rect key={lm.key} x={x - 1} y={y - 1} width={2} height={6} fill={c} opacity={o} />;
        return <rect key={lm.key} x={x} y={y} width={1} height={4} fill={c} opacity={o} />;
      })}
      <rect x={wagonX - 2} y={y - 2} width={5} height={4} fill={sc.fg} />
      <rect x={wagonX - 3} y={y} width={1} height={2} fill={sc.fg} />
      <rect x={wagonX + 2} y={y} width={1} height={2} fill={sc.fg} />
    </g>
  );
}

// ---- measuring the readout face ------------------------------------------------
// Themes choose the readout font, and their widths differ a lot (VT323 is
// 0.4em a character, a typical monospace 0.6em). A fixed column count either
// wastes a third of the screen or runs off it, so text cabinets ask how many
// characters fit a width at a size, measured from the live font.

/** Assumed when the font can't be measured (tests, no canvas). */
const FALLBACK_ADVANCE = 0.6;
let measured: { key: string; advance: number } | null = null;

/** The readout face's character advance, in em. */
export function readoutAdvance(): number {
  if (typeof document === "undefined" || /jsdom/i.test(globalThis.navigator?.userAgent ?? "")) return FALLBACK_ADVANCE;
  try {
    const family = getComputedStyle(document.documentElement).getPropertyValue("--font-readout").trim() || "monospace";
    // Re-measure once web fonts finish loading: before that the fallback
    // face answers, and its width isn't the pixel font's.
    const key = `${family}|${document.fonts?.status ?? ""}`;
    if (measured?.key === key) return measured.advance;
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return FALLBACK_ADVANCE;
    ctx.font = `100px ${family}`;
    const advance = ctx.measureText("abcdefghij0123456789").width / 20 / 100;
    measured = { key, advance: advance > 0.2 && advance < 1 ? advance : FALLBACK_ADVANCE };
    return measured.advance;
  } catch {
    return FALLBACK_ADVANCE;
  }
}

/** How many readout characters of `size` fit in `width` viewBox units. */
export function textCols(width: number, size: number): number {
  return Math.max(20, Math.floor(width / (size * readoutAdvance() * 1.03)));
}

/** Greedy word wrap for the readout face, which is close to monospace at the
    sizes the cabinets use: `cols` is the character budget per line. Words
    longer than a line are split so nothing runs off the screen. */
export function wrapText(text: string, cols: number, maxLines = Infinity): string[] {
  const lines: string[] = [];
  for (const paragraph of text.split(/\r?\n/)) {
    let line = "";
    for (const word of paragraph.split(/\s+/).filter(Boolean)) {
      let rest = word;
      while (rest.length > cols) {
        if (line) {
          lines.push(line);
          line = "";
        }
        lines.push(rest.slice(0, cols));
        rest = rest.slice(cols);
      }
      if (!line) line = rest;
      else if (line.length + 1 + rest.length <= cols) line += " " + rest;
      else {
        lines.push(line);
        line = rest;
      }
    }
    lines.push(line);
  }
  while (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  if (lines.length > maxLines) {
    const cut = lines.slice(0, maxLines);
    cut[maxLines - 1] = cut[maxLines - 1].slice(0, Math.max(0, cols - 1)) + "…";
    return cut;
  }
  return lines;
}

// ---- the all-time table ------------------------------------------------------
// Each cabinet keeps a top-5 in localStorage, seeded with names so a first run
// has something to beat. `loadTable` falls back to the seed on a missing or
// corrupt entry; `recordScore` inserts a finished run and reports its rank.

export interface ScoreEntry {
  name: string;
  score: number;
  /** A one-word qualifier shown beside the score (an occupation, a mode). */
  occupation: string;
}

export function loadTable(key: string, seed: ScoreEntry[]): ScoreEntry[] {
  try {
    const raw = loadPref(key);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed) && parsed.length) return parsed;
    }
  } catch {
    // fall through to the seed
  }
  return seed;
}

/** Insert a finished run; returns its 1-based rank, or 0 if it didn't place. */
export function recordScore(key: string, seed: ScoreEntry[], score: number, occupation: string): number {
  const table = [...loadTable(key, seed), { name: "You", score, occupation }].sort((a, b) => b.score - a.score).slice(0, 5);
  savePref(key, JSON.stringify(table));
  const i = table.findIndex((e) => e.name === "You" && e.score === score);
  return i < 0 ? 0 : i + 1;
}

export const rankWord = (rank: number) => (rank === 1 ? "#1 of all time!" : `#${rank} of all time`);
