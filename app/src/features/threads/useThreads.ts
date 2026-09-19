// The one place thread verdicts are derived from store state, shared by every
// surface that paints a chat's standing (tabs, history, project cards, the
// project's chat list). A chat can be reached without ever visiting Home, so
// this fetches the snapshot once if nothing has; the store keeps it fresh.

import { useEffect, useMemo } from "react";
import { useStore } from "../../lib/store";
import { threadsFromState, type Thread } from "./threads";

export function useThreads(): Map<string, Thread> {
  const threadsSnapshot = useStore((s) => s.threadsSnapshot);
  const refreshThreads = useStore((s) => s.refreshThreads);
  const runStatus = useStore((s) => s.runStatus);
  const approvals = useStore((s) => s.approvals);
  useEffect(() => {
    if (!threadsSnapshot) void refreshThreads();
  }, [threadsSnapshot, refreshThreads]);
  return useMemo(
    () => threadsFromState({ threadsSnapshot, runStatus, approvals }),
    [threadsSnapshot, runStatus, approvals],
  );
}
