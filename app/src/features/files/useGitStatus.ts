// Keep the store's git status fresh for a workspace, from whichever surface
// is mounted (the Files tree, the editor pane, or both — a duplicate refresh
// is just a second cheap `git status`). Refreshes on mount/workspace change,
// when a turn ends (the agent writes files), on watcher batches, and on
// window focus — the watcher deliberately drops `.git`, so a commit made in a
// terminal only surfaces when the user comes back to the app.

import { useCallback, useEffect, useRef } from "react";
import { useStore } from "../../lib/store";
import type { GitFileState } from "../../lib/types";

export function useGitStatus(workspace: string | null): GitFileState[] | null {
  const states = useStore((s) => (workspace ? (s.gitStates[workspace] ?? null) : null));
  const refresh = useStore((s) => s.refreshGitStatus);
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  const fsChange = useStore((s) => s.fsChange);

  const load = useCallback(() => {
    if (workspace) void refresh(workspace);
  }, [workspace, refresh]);

  useEffect(() => {
    load();
  }, [load]);

  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) load();
    wasRunning.current = running;
  }, [running, load]);

  useEffect(() => {
    if (fsChange && fsChange.root === workspace) load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fsChange]);

  useEffect(() => {
    const onFocus = () => load();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);

  return states;
}
