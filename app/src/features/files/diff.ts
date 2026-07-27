// Diff tabs and unified-diff parsing. A diff rides the existing editor tab
// model as a marked path, so opening, fronting, and closing all reuse the
// store's tab plumbing untouched. The marker is checked BEFORE any extension
// sniffing (a diff tab for photo.png is a diff, not an image).

import type { GitStatusKind } from "../../lib/types";

// The marker starts with NUL — the one byte no POSIX filename can contain —
// so no real workspace file can ever collide with it (a file literally named
// `diff:notes.md` is a valid filename and must open as a FILE). Tab paths are
// in-memory only; the marker never reaches disk or the backend.
const DIFF_PREFIX = "\u0000diff:";

/** The editor-tab path that shows the diff for a workspace file. */
export const diffTab = (path: string) => DIFF_PREFIX + path;

/** Whether an editor-tab path is a diff tab. */
export const isDiffPath = (path: string) => path.startsWith(DIFF_PREFIX);

/** The workspace-relative file a diff tab shows. */
export const diffTarget = (path: string) => path.slice(DIFF_PREFIX.length);

/** The single letter shown in badges, VS Code style. */
export const statusLetter: Record<GitStatusKind, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  untracked: "U",
  conflicted: "C",
};

/** One rendered line of a unified diff. */
export interface DiffRow {
  kind: "meta" | "hunk" | "add" | "del" | "ctx";
  /** Line number in the old file (null for added/meta/hunk lines). */
  oldNo: number | null;
  /** Line number in the new file (null for deleted/meta/hunk lines). */
  newNo: number | null;
  /** The line's text without its +/-/space marker (meta/hunk keep it all). */
  text: string;
}

/** Parse `git diff` output into rows with old/new line numbers, ready to
 *  render. Tolerant of anything unexpected: unknown lines become meta rows,
 *  never a throw — worst case the diff shows as plain text. */
export function parseUnifiedDiff(diff: string): DiffRow[] {
  const rows: DiffRow[] = [];
  let oldNo = 0;
  let newNo = 0;
  let inHunk = false;
  for (const line of diff.split("\n")) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      oldNo = parseInt(hunk[1], 10);
      newNo = parseInt(hunk[2], 10);
      inHunk = true;
      rows.push({ kind: "hunk", oldNo: null, newNo: null, text: line });
      continue;
    }
    if (!inHunk || line.startsWith("diff --git")) {
      inHunk = false;
      if (line) rows.push({ kind: "meta", oldNo: null, newNo: null, text: line });
      continue;
    }
    if (line.startsWith("+")) {
      rows.push({ kind: "add", oldNo: null, newNo: newNo++, text: line.slice(1) });
    } else if (line.startsWith("-")) {
      rows.push({ kind: "del", oldNo: oldNo++, newNo: null, text: line.slice(1) });
    } else if (line.startsWith("\\")) {
      // "\ No newline at end of file"
      rows.push({ kind: "meta", oldNo: null, newNo: null, text: line });
    } else {
      rows.push({ kind: "ctx", oldNo: oldNo++, newNo: newNo++, text: line.slice(1) });
    }
  }
  // A trailing empty context row is the split artifact of the final newline.
  const last = rows[rows.length - 1];
  if (last?.kind === "ctx" && last.text === "") rows.pop();
  return rows;
}
