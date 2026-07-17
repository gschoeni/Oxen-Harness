import { useEffect } from "react";
import { useStore } from "../../lib/store";

/** Run `onChange` when a watcher batch touches any of `paths` in this
 *  workspace (an empty batch means "bulk change" and always matches). */
export function useFsChanged(workspace: string, paths: string[], onChange: () => void) {
  const fsChange = useStore((s) => s.fsChange);
  useEffect(() => {
    if (!fsChange || fsChange.root !== workspace) return;
    if (fsChange.paths.length && !paths.some((p) => fsChange.paths.includes(p))) return;
    onChange();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fsChange]);
}
