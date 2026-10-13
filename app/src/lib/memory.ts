// What local models cost the machine's memory, for the memory meter, the
// notice banner, and Settings. One poller feeds every reader: every 2s while
// a local model server runs (its footprint moves while it loads and
// renders), every 10s otherwise (so a server starting is noticed). A
// `memory://notice` — a load warned about or refused, a model the watchdog
// stopped — refreshes at once and queues a banner.

import { useEffect } from "react";
import { create } from "zustand";
import { memoryStatus, onMemoryNotice } from "./ipc";
import type { MemoryNotice, MemoryStatus, MemorySystem } from "./types";

/** How long a notice keeps the meter visible after the model is gone. */
export const NOTICE_FRESH_SECS = 5 * 60;
const ACTIVE_POLL_MS = 2000;
const IDLE_POLL_MS = 10_000;

interface MemoryState {
  status: MemoryStatus | null;
  /** Notices waiting in the banner, oldest first. */
  banners: MemoryNotice[];
  refresh: () => Promise<void>;
  noticed: (n: MemoryNotice) => void;
  dismiss: (n: MemoryNotice) => void;
}

export const useMemory = create<MemoryState>((set, get) => ({
  status: null,
  banners: [],
  refresh: async () => {
    try {
      set({ status: await memoryStatus() });
    } catch {
      // A failed read keeps the last one; the next poll tries again.
    }
  },
  noticed: (n) => {
    set((s) => ({
      banners: [...s.banners.filter((b) => !(b.model === n.model && b.level === n.level)), n],
    }));
    void get().refresh();
  },
  dismiss: (n) => set((s) => ({ banners: s.banners.filter((b) => b !== n) })),
}));

/** Whether anything local is worth showing: a running model server, or a
 *  notice from the last few minutes. */
export function memoryActive(status: MemoryStatus | null, nowSecs = Date.now() / 1000): boolean {
  if (!status?.system) return false;
  if (status.processes.length > 0) return true;
  return !!status.notice && nowSecs - status.notice.at < NOTICE_FRESH_SECS;
}

/** Drive the shared poller and the notice listener. Mount once (the chat). */
export function useMemoryPolling() {
  const refresh = useMemory((s) => s.refresh);
  const noticed = useMemory((s) => s.noticed);
  const active = useMemory((s) => memoryActive(s.status));
  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), active ? ACTIVE_POLL_MS : IDLE_POLL_MS);
    return () => clearInterval(t);
  }, [active, refresh]);
  useEffect(() => {
    let off: (() => void) | undefined;
    let dead = false;
    void onMemoryNotice(noticed).then((un) => {
      if (dead) un();
      else off = un;
    });
    return () => {
      dead = true;
      off?.();
    };
  }, [noticed]);
}

/** `30.1` — bytes as decimal gigabytes, one place. */
export function gb(bytes: number): string {
  return (bytes / 1e9).toFixed(1);
}

/** What the pressure reading means to a person. */
export function pressureLabel(p: MemorySystem["pressure"]): string {
  switch (p) {
    case "normal":
      return "Normal";
    case "warn":
      return "High";
    case "critical":
      return "Critical";
    default:
      return "Unknown";
  }
}

/** The total footprint of the app's local model servers. */
export function localFootprint(status: MemoryStatus): number {
  return status.processes.reduce((sum, p) => sum + p.footprint_bytes, 0);
}
