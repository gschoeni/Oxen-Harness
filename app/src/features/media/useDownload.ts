// "Download": copy a file into the user's Downloads folder. One click,
// no dialog — the file already has a readable name, and the answer to "where
// did it go" is the folder every other download lands in. The outcome shows
// on the button itself for a moment, then it is ready to go again.

import { useCallback, useEffect, useRef, useState } from "react";
import { fsDownload } from "../../lib/ipc";

export type DownloadState = "idle" | "saving" | "saved" | "failed";

/** How long "Saved" (or the failure) stays on the button. */
export const DOWNLOAD_NOTE_MS = 2500;

export function useDownload(workspace: string, path: string | null | undefined) {
  const [state, setState] = useState<DownloadState>("idle");
  /** Where the copy landed, or why it didn't. */
  const [detail, setDetail] = useState("");
  const timer = useRef<number | undefined>(undefined);

  // The detail view steps between generations without remounting: the note
  // belongs to the file it was about.
  useEffect(() => {
    setState("idle");
    setDetail("");
    return () => window.clearTimeout(timer.current);
  }, [workspace, path]);

  const download = useCallback(async () => {
    if (!path) return;
    window.clearTimeout(timer.current);
    setState("saving");
    try {
      setDetail(await fsDownload(workspace, path));
      setState("saved");
    } catch (e) {
      setDetail(String(e));
      setState("failed");
    }
    timer.current = window.setTimeout(() => setState("idle"), DOWNLOAD_NOTE_MS);
  }, [workspace, path]);

  return { state, detail, download };
}

/** The button's words for each state. */
export function downloadLabel(state: DownloadState): string {
  switch (state) {
    case "saving":
      return "Downloading…";
    case "saved":
      return "Saved to Downloads";
    case "failed":
      return "Download failed";
    default:
      return "Download";
  }
}
