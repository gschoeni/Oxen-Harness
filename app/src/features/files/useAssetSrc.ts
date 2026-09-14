// The one sanctioned way to put a workspace file's bytes on screen: resolve
// the path through `fs_asset_path` (canonicalized and symlink-checked against
// the workspace boundary) and hand the result to the asset protocol. Shared by
// the Editor dock's media views, the chat's generation cards, and the Gallery.

import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { fsAssetPath } from "../../lib/ipc";

/** Asset-protocol URL for a workspace file, gated on the backend's CANONICAL
 *  boundary check: the asset protocol follows symlinks, so a workspace entry
 *  linking outside the project must yield nothing, not someone's home file.
 *  Null until validated (and forever, for anything outside). */
export function useAssetSrc(workspace: string, path: string, bust: number): string | null {
  const [abs, setAbs] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    setAbs(null);
    fsAssetPath(workspace, path)
      .then((real) => {
        if (!stale) setAbs(real);
      })
      .catch(() => {
        /* outside the boundary (or gone): render nothing */
      });
    return () => {
      stale = true;
    };
  }, [workspace, path]);
  return abs ? convertFileSrc(abs) + (bust ? `?v=${bust}` : "") : null;
}

