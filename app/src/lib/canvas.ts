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

/** The canvas format a project file renders as — mirrors
 *  `harness_tools::canvas::format_for_path`. */
export function formatForPath(path: string): CanvasFormat {
  const ext = path.split("/").pop()?.split(".").pop()?.toLowerCase() ?? "";
  if (["md", "markdown", "mdx"].includes(ext)) return "markdown";
  if (["html", "htm"].includes(ext)) return "html";
  if (ext === "svg") return "svg";
  return "code";
}

/** Build a CanvasDoc from parsed `canvas` tool args, or null if there's
 *  nothing to show (a malformed/partial call). A call that showed a project
 *  file carries no content: the panel reads the file itself. */
export function canvasDocFromArgs(a: Record<string, unknown>): CanvasDoc | null {
  const content = typeof a.content === "string" ? a.content : "";
  const path = typeof a.path === "string" && a.path.trim() ? a.path.trim().replace(/^\.\//, "") : null;
  if (!content.trim() && !path) return null;
  const fileName = path?.split("/").pop();
  const title =
    typeof a.title === "string" && a.title.trim() ? a.title.trim() : (fileName ?? "Document");
  const format = (typeof a.format === "string" ? a.format : path ? formatForPath(path) : "markdown") as CanvasFormat;
  const language = typeof a.language === "string" ? a.language : undefined;
  const idSource = typeof a.id === "string" && a.id.trim() ? a.id : (path ?? title);
  return { id: slugId(idSource), title, format, language, content, path };
}
