/** The themed "thinking" voice: which phrases/glyphs animate while the model
 *  works, and the cadence they rotate at. The pools come straight from the
 *  active harness theme (`voice.thinking`, `voice.spinner_glyphs`,
 *  `voice.tool_verbs`) — the same data the CLI spinner speaks from — and the
 *  cadence mirrors `crates/harness-theme/src/spinner.rs` (FRAME_MS /
 *  FRAMES_PER_PHRASE) so both hosts breathe at the same rhythm. Change it
 *  there, change it here. */

import type { Theme } from "./types";

/** Milliseconds per animation frame (one glyph step). */
export const FRAME_MS = 110;
/** A new phrase every this many frames (~1.8s). */
export const FRAMES_PER_PHRASE = 16;

/** Last-resort pools for the moment before a theme loads (Oregon Trail's). */
const FALLBACK_GLYPHS = ["✶", "✸", "✺", "✹", "✷", "✦"];
const FALLBACK_PHRASES = ["Thinking"];

/** Phrases shown while the model is thinking (no text streamed yet). */
export function thinkingPhrases(theme: Theme | null): string[] {
  const pool = theme?.voice?.thinking;
  return pool && pool.length > 0 ? pool : FALLBACK_PHRASES;
}

/** Phrases shown while the model is actively writing (a pause mid-text).
 *  Mirrors the CLI's `Ui::writing`: the theme's `write_file` verbs when
 *  present, otherwise the thinking phrases so the indicator is never empty. */
export function writingPhrases(theme: Theme | null): string[] {
  const verbs = theme?.voice?.tool_verbs?.["write_file"];
  return verbs && verbs.length > 0 ? verbs : thinkingPhrases(theme);
}

/** The theme's spinner glyph cycle. */
export function spinnerGlyphs(theme: Theme | null): string[] {
  const pool = theme?.voice?.spinner_glyphs;
  return pool && pool.length > 0 ? pool : FALLBACK_GLYPHS;
}

/** The phrase showing at `frame`, having opened on `startIndex` — same
 *  rotation as the Rust `Rhythm`: advance once every FRAMES_PER_PHRASE. */
export function phraseAt(pool: string[], startIndex: number, frame: number): string {
  if (pool.length === 0) return "";
  const steps = Math.floor(frame / FRAMES_PER_PHRASE);
  return pool[(startIndex + steps) % pool.length];
}

/** The glyph showing at `frame` (one step per frame). */
export function glyphAt(pool: string[], frame: number): string {
  if (pool.length === 0) return "";
  return pool[frame % pool.length];
}

/** Format elapsed ms like the CLI spinner's timer: `7s`, `1m07s`. */
export function elapsedLabel(ms: number): string {
  const secs = Math.max(0, Math.floor(ms / 1000));
  if (secs < 60) return `${secs}s`;
  return `${Math.floor(secs / 60)}m${String(secs % 60).padStart(2, "0")}s`;
}
