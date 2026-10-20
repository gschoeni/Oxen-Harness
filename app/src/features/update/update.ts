// App updates: one store behind the title-bar pill and Settings → Updates.
// The launch check reuses an answer up to an hour old (the backend caches
// it), and a long-open window asks again every few hours, so a release
// published mid-day still shows up without a relaunch.

import { useEffect } from "react";
import { create } from "zustand";
import { checkForUpdate, installUpdate } from "../../lib/ipc";
import type { UpdateStatus } from "../../lib/types";

const RECHECK_MS = 6 * 60 * 60 * 1000;

interface UpdateState {
  status: UpdateStatus | null;
  checking: boolean;
  installing: boolean;
  /** Why the last check or install failed, for Settings → Updates. */
  error: string | null;
  check: (force: boolean) => Promise<void>;
  install: () => Promise<void>;
}

export const useUpdate = create<UpdateState>((set, get) => ({
  status: null,
  checking: false,
  installing: false,
  error: null,
  check: async (force) => {
    if (get().checking) return;
    set({ checking: true });
    try {
      set({ status: await checkForUpdate(force), error: null });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ checking: false });
    }
  },
  install: async () => {
    if (get().installing) return;
    set({ installing: true, error: null });
    try {
      // Success relaunches the app, so returning at all means it didn't.
      await installUpdate();
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ installing: false });
    }
  },
}));

/** Check at launch and every few hours after. Mount once (the title bar). */
export function useUpdateCheck() {
  const check = useUpdate((s) => s.check);
  useEffect(() => {
    void check(false);
    const timer = window.setInterval(() => void check(false), RECHECK_MS);
    return () => window.clearInterval(timer);
  }, [check]);
}

/** "Oct 20, 2026" for a release's ISO timestamp. */
export function releaseDate(iso: string | null): string | null {
  if (!iso) return null;
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? null
    : date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}
