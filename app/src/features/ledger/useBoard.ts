// The one place the board is derived from store state, shared by every
// surface that renders threads (the Ledger page, a project's trail section).
// Null until the first snapshot lands.

import { useMemo } from "react";
import { useStore } from "../../lib/store";
import { deriveBoard, type Board } from "./ledger";

export function useBoard(): Board | null {
  const ledger = useStore((s) => s.ledger);
  const ledgerGit = useStore((s) => s.ledgerGit);
  const projects = useStore((s) => s.projects);
  const runStatus = useStore((s) => s.runStatus);
  const approvals = useStore((s) => s.approvals);

  return useMemo(() => {
    if (!ledger) return null;
    // The snapshot's running set survives restarts; live run statuses cover
    // turns started since it was taken. Union, never either alone.
    const running = new Set(ledger.running);
    for (const [id, status] of Object.entries(runStatus)) {
      if (status === "running") running.add(id);
    }
    // A pending approval means the agent is parked mid-turn on the user.
    const waiting = new Set(
      Object.entries(approvals)
        .filter(([, request]) => request !== undefined)
        .map(([session]) => session),
    );
    return deriveBoard({
      entries: ledger.entries,
      running,
      waiting,
      lastSeen: ledger.last_seen,
      projects,
      git: ledgerGit,
      now: Math.floor(Date.now() / 1000),
    });
  }, [ledger, runStatus, approvals, projects, ledgerGit]);
}
