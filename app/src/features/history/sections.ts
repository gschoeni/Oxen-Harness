// The order every chat list shows a project's chats in, top to bottom: the
// ones that need the user (most urgent first — a parked agent, a lost reply,
// a finish they haven't seen, an open plan, a thread going cold), the rest of
// the open chats in their usual order, and the tied-off ones last.

import { needLabel, needRank, needsUser, rankOfNeed, type Thread } from "../ledger/ledger";
import { basename } from "../../lib/format";
import type { RunStatus, SessionSummary } from "../../lib/types";

/** A needy chat with the words it wears. */
export interface NeedyRow {
  row: SessionSummary;
  label: string;
}

export interface Sections {
  needs: NeedyRow[];
  open: SessionSummary[];
  settled: SessionSummary[];
}

/** Reads the board's verdict per thread; a chat the board hasn't met yet (no
 *  user turn, or the snapshot still loading) is an ordinary open chat. A
 *  finish the store saw land offscreen this session (`unread`) counts as
 *  needing the user even before the board catches up — the dot always meant
 *  "look at this". */
export function sectionRows(
  rows: SessionSummary[],
  threads: Map<string, Thread>,
  runStatus: Record<string, RunStatus | undefined>,
): Sections {
  const needs: (NeedyRow & { rank: number; index: number })[] = [];
  const open: SessionSummary[] = [];
  const settled: SessionSummary[] = [];
  rows.forEach((row, index) => {
    const thread = threads.get(row.id);
    if (thread?.state === "settled") {
      settled.push(row);
    } else if (thread && needsUser(thread)) {
      needs.push({ row, label: needLabel(thread) ?? "", rank: needRank(thread), index });
    } else if (runStatus[row.id] === "unread") {
      needs.push({ row, label: "finished while you were away", rank: rankOfNeed("finished"), index });
    } else {
      open.push(row);
    }
  });
  needs.sort((a, b) => a.rank - b.rank || a.index - b.index);
  return { needs: needs.map(({ row, label }) => ({ row, label })), open, settled };
}

/** Whether a chat matches a free-text query: every whitespace-separated term
 *  must appear somewhere in its title, model, project name or folder, or id.
 *  Case-blind. The folder's full path is left out on purpose: every chat
 *  under ~/Code would match "code". */
export function matchesQuery(row: SessionSummary, projectName: string, query: string): boolean {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return true;
  const hay = [row.title ?? "", row.model, projectName, basename(row.workspace), row.id]
    .join(" ")
    .toLowerCase();
  return terms.every((term) => hay.includes(term));
}
