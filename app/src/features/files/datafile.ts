// Pure helpers behind the DataView grid — path routing, page math, and the
// cell-value round trip (display formatting and edit parsing), kept free of
// React so they're trivially testable.

import type { DatasetKind } from "../../lib/types";

/** Rows fetched per backend request. One page ≈ a few screenfuls. */
export const PAGE_SIZE = 200;

export type CellValue = string | number | boolean | null;

const DATA_EXTENSIONS = new Set(["csv", "tsv", "jsonl", "ndjson", "parquet"]);

/** Files the data grid opens (everything else falls through to the editor). */
export function isDataPath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  return dot >= 0 && DATA_EXTENSIONS.has(path.slice(dot + 1).toLowerCase());
}

/** The page a view row lives on. */
export const pageOf = (row: number) => Math.floor(row / PAGE_SIZE);

/** Cell → the string shown in the grid. */
export function formatCell(value: CellValue, kind: DatasetKind): string {
  if (value === null) return "";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number" && kind === "float" && !Number.isInteger(value)) {
    // Full precision is preserved in the file; the grid shows a sane width.
    return String(Math.round(value * 1e6) / 1e6);
  }
  return String(value);
}

/** Cell → the string offered for editing (exact, never rounded). */
export function editText(value: CellValue): string {
  if (value === null) return "";
  return String(value);
}

/** Parse what the user typed into the JSON value written to the file,
 *  honoring the column's dtype. Returns `undefined` when the text can't
 *  become that type (the edit is rejected, not silently mangled). */
export function parseEdit(text: string, kind: DatasetKind): CellValue | undefined {
  const trimmed = text.trim();
  switch (kind) {
    case "int": {
      if (trimmed === "") return null;
      if (!/^[+-]?\d+$/.test(trimmed)) return undefined;
      const n = Number(trimmed);
      return Number.isSafeInteger(n) ? n : undefined;
    }
    case "float": {
      if (trimmed === "") return null;
      const n = Number(trimmed);
      return Number.isFinite(n) ? n : undefined;
    }
    case "bool": {
      if (trimmed === "") return null;
      if (/^(true|1|yes)$/i.test(trimmed)) return true;
      if (/^(false|0|no)$/i.test(trimmed)) return false;
      return undefined;
    }
    case "list":
    case "struct":
      return undefined; // nested values are read-only
    default:
      // Strings (and temporal columns, which the backend casts) take the
      // text as-is; an empty cell means null, not "".
      return text === "" ? null : text;
  }
}

/** A column's initial width, from its name and a sample of its values. */
export function initialWidth(name: string, kind: DatasetKind, samples: string[]): number {
  const longest = samples.reduce((max, s) => Math.max(max, s.length), name.length);
  const numeric = kind === "int" || kind === "float";
  const perChar = numeric ? 8.5 : 7.5;
  const width = Math.ceil(longest * perChar) + 28;
  return Math.min(Math.max(width, 88), 360);
}

/** The row-number gutter width, sized to the biggest row number shown. */
export function gutterWidth(totalRows: number): number {
  return Math.max(44, 20 + String(Math.max(totalRows, 1)).length * 8);
}
