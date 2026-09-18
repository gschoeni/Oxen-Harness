// The one place the board is derived from store state, shared by every
// surface that renders threads (the Ledger page, a project's trail section).
// Null until the first snapshot lands.

import { useEffect, useMemo } from "react";
import { useStore } from "../../lib/store";
import { boardFromState, type Board, type Thread } from "./ledger";

export function useBoard(): Board | null {
  const ledger = useStore((s) => s.ledger);
  const ledgerGit = useStore((s) => s.ledgerGit);
  const projects = useStore((s) => s.projects);
  const runStatus = useStore((s) => s.runStatus);
  const approvals = useStore((s) => s.approvals);

  return useMemo(
    () => boardFromState({ ledger, ledgerGit, projects, runStatus, approvals }),
    [ledger, runStatus, approvals, projects, ledgerGit],
  );
}

/** Every thread the board has a verdict on, by session — the lookup the tab
 *  strip and the history rows share. A chat can be reached without ever
 *  visiting Home, so this fetches the snapshot once if nothing has; the store
 *  keeps it fresh from there. Archived ("lost") threads are left out on
 *  purpose: the amnesty means they never nag. */
export function useThreads(): Map<string, Thread> {
  const ledger = useStore((s) => s.ledger);
  const refreshLedger = useStore((s) => s.refreshLedger);
  useEffect(() => {
    if (!ledger) void refreshLedger();
  }, [ledger, refreshLedger]);
  const board = useBoard();
  return useMemo(() => {
    const map = new Map<string, Thread>();
    for (const train of board?.trains ?? []) {
      for (const thread of train.threads) map.set(thread.entry.id, thread);
    }
    for (const thread of board?.settled ?? []) map.set(thread.entry.id, thread);
    return map;
  }, [board]);
}
