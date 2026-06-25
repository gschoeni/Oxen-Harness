// Reconstruct a CanvasDoc from a `canvas` tool call's raw arguments. Because the
// document content lives in the tool call (which is part of the chat transcript),
// any past canvas — including ones from a resumed, previously-saved chat — can be
// re-opened straight from its tool-call chip.

import type { CanvasDoc, CanvasFormat } from "./types";

/** Filesystem/anchor-safe slug — mirrors `harness_tools::canvas::slug` exactly so
 *  a reconstructed id matches the one the backend derived (updates collapse). */
export function slugId(s: string): string {
  let out = "";
  for (const ch of s.trim()) {
    out += /[a-zA-Z0-9]/.test(ch) ? ch.toLowerCase() : "-";
  }
  out = out.replace(/^-+|-+$/g, "");
  if (!out) return "document";
  return Array.from(out).slice(0, 64).join("");
}

/** Build a CanvasDoc from parsed `canvas` tool args, or null if there's no
 *  content (e.g. a malformed/partial call). */
export function canvasDocFromArgs(a: Record<string, unknown>): CanvasDoc | null {
  const content = typeof a.content === "string" ? a.content : "";
  if (!content.trim()) return null;
  const title =
    typeof a.title === "string" && a.title.trim() ? a.title.trim() : "Document";
  const format = (typeof a.format === "string" ? a.format : "markdown") as CanvasFormat;
  const language = typeof a.language === "string" ? a.language : undefined;
  const idSource = typeof a.id === "string" && a.id.trim() ? a.id : title;
  return { id: slugId(idSource), title, format, language, content };
}
