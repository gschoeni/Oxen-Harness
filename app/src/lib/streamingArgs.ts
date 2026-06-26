// Helpers for showing a tool call's content while its JSON arguments are still
// streaming in. The model emits arguments character-by-character, so the buffer
// is almost always incomplete JSON (an unterminated string). `JSON.parse` can't
// handle that, so we extract a single string field's in-progress value directly,
// decoding escapes and tolerating a string cut off mid-stream.

import type { CanvasDoc, CanvasFormat } from "./types";
import { slugId } from "./canvas";

const ESCAPES: Record<string, string> = {
  n: "\n",
  t: "\t",
  r: "\r",
  b: "\b",
  f: "\f",
  '"': '"',
  "\\": "\\",
  "/": "/",
};

/** Best-effort value of a string field in (possibly partial) JSON, or null if
 *  the field/opening quote hasn't streamed in yet. Handles escapes and a string
 *  that's still being written (no closing quote). */
export function extractStringField(partial: string, key: string): string | null {
  const marker = `"${key}"`;
  let i = partial.indexOf(marker);
  if (i < 0) return null;
  i += marker.length;
  while (i < partial.length && /\s/.test(partial[i])) i++;
  if (partial[i] !== ":") return null;
  i++;
  while (i < partial.length && /\s/.test(partial[i])) i++;
  if (partial[i] !== '"') return null;
  i++;

  let out = "";
  while (i < partial.length) {
    const ch = partial[i];
    if (ch === '"') break; // closing quote → value complete
    if (ch === "\\") {
      const next = partial[i + 1];
      if (next === undefined) break; // escape cut off at the stream edge
      if (next === "u") {
        const hex = partial.slice(i + 2, i + 6);
        if (hex.length < 4) break; // incomplete \uXXXX at the edge
        out += String.fromCharCode(parseInt(hex, 16) || 0);
        i += 6;
        continue;
      }
      out += ESCAPES[next] ?? next;
      i += 2;
      continue;
    }
    out += ch;
    i++;
  }
  return out;
}

// Map a file extension to a highlight.js language name (aliases hljs may not
// resolve directly). Anything unmapped falls back to auto-detection.
const EXT_LANG: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  py: "python",
  rb: "ruby",
  go: "go",
  java: "java",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  swift: "swift",
  kt: "kotlin",
  scala: "scala",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  json: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "ini",
  ini: "ini",
  css: "css",
  scss: "scss",
  less: "less",
  html: "xml",
  xml: "xml",
  vue: "xml",
  svg: "xml",
  md: "markdown",
  markdown: "markdown",
  sql: "sql",
  dockerfile: "dockerfile",
};

/** A highlight.js language for a path, by extension (or undefined → auto-detect). */
export function langForPath(path: string | null | undefined): string | undefined {
  if (!path) return undefined;
  const ext = path.split(/[./\\]/).pop()?.toLowerCase();
  return ext ? EXT_LANG[ext] : undefined;
}

export interface StreamingWrite {
  /** "Writing" (write_file) or "Editing" (edit_file). */
  verb: string;
  path: string | null;
  content: string;
  language?: string;
}

/** Extract the in-progress file content from a write_file/edit_file call's
 *  partial args, or null for any other tool. */
export function partialFileWrite(name: string, args: string): StreamingWrite | null {
  if (name === "write_file") {
    const path = extractStringField(args, "path");
    return { verb: "Writing", path, content: extractStringField(args, "contents") ?? "", language: langForPath(path) };
  }
  if (name === "edit_file") {
    const path = extractStringField(args, "path");
    return { verb: "Editing", path, content: extractStringField(args, "new_string") ?? "", language: langForPath(path) };
  }
  return null;
}

/** Build a provisional CanvasDoc from a canvas call's partial args, or null if
 *  no content has streamed yet. Mirrors `canvasDocFromArgs` but tolerant of the
 *  in-progress buffer. */
export function partialCanvasDoc(args: string): CanvasDoc | null {
  const content = extractStringField(args, "content");
  if (content == null) return null;
  const title = extractStringField(args, "title")?.trim() || "Document";
  const format = (extractStringField(args, "format") || "markdown") as CanvasFormat;
  const language = extractStringField(args, "language") || undefined;
  const idSource = extractStringField(args, "id")?.trim() || title;
  return { id: slugId(idSource), title, format, language, content };
}
