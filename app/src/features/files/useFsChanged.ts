import { useEffect, useRef } from "react";
import { useStore } from "../../lib/store";

/** Run `onChange` when a watcher batch touches any of `paths` in this
 *  workspace (an empty batch means "bulk change" and always matches).
 *  Only batches that arrive after mount count — the store keeps the last
 *  event around, and replaying it would trigger a spurious reload. */
export function useFsChanged(workspace: string, paths: string[], onChange: () => void) {
  const fsChange = useStore((s) => s.fsChange);
  const seenTick = useRef(useStore.getState().fsChange?.tick ?? 0);
  useEffect(() => {
    if (!fsChange || fsChange.tick === seenTick.current) return;
    seenTick.current = fsChange.tick;
    if (fsChange.root !== workspace) return;
    if (fsChange.paths.length && !paths.some((p) => fsChange.paths.includes(p))) return;
    onChange();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fsChange]);
}
