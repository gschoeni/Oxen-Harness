// UI preferences (color mode, dock layout, home view, …) persisted to
// `~/.oxen-harness/ui.json` instead of the webview's localStorage, so the
// harness base dir is the single home for all app state (`OXEN_HARNESS_DIR`
// resets/relocates everything at once).
//
// The file is loaded once at boot (`initUiState`, awaited in `main.tsx` before
// anything that reads prefs at module-init time is imported) into a module
// cache, so reads stay synchronous. Writes update the cache and persist the
// whole object on a short debounce, flushed on unload.

import { loadUiState, saveUiState } from "./ipc";

/** Keys the app persists. One flat object — the backend stores it opaquely. */
export interface UiState {
  /** Color mode ("light" | "dark"); absent → follow the OS. */
  mode?: string;
  /** Selected hero game for the empty state. */
  heroGame?: string;
  /** Dock layout: per-side widths + collapsed sides. */
  docks?: { widths: Record<string, number>; collapsed: Record<string, boolean> };
  /** Home's lens: "ledger" | "cards". */
  homeView?: string;
  /** Project-cards ordering: "recent" | "name". */
  projectsSort?: string;
}

let cache: UiState = {};
let saveTimer: ReturnType<typeof setTimeout> | null = null;

/** localStorage keys this state lived under before it moved to ui.json. */
const LEGACY_KEYS: Record<keyof UiState, string> = {
  mode: "oxen-ui-mode",
  heroGame: "oxen-hero-game",
  docks: "oxen-docks",
  homeView: "oxen-harness.home-view",
  projectsSort: "oxen-harness.projects-sort",
};

/** Load ui.json into the cache. Awaited in `main.tsx` before the store module
 *  is imported; a missing/broken file (or backend) just means defaults. On
 *  first run, adopts any prefs left behind in localStorage by older builds. */
export async function initUiState(): Promise<void> {
  try {
    const saved = await loadUiState();
    cache = (saved as UiState) ?? migrateFromLocalStorage();
  } catch {
    cache = {};
  }
  // A hard quit right after a change would lose the debounced write.
  window.addEventListener("beforeunload", flushUiState);
}

/** Read a preference from the boot-loaded cache. */
export function getUi<K extends keyof UiState>(key: K): UiState[K] {
  return cache[key];
}

/** Write a preference: updates the cache now, persists soon. */
export function setUi<K extends keyof UiState>(key: K, value: UiState[K]): void {
  cache[key] = value;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(flushUiState, 300);
}

/** Persist the cache immediately (a failed save must never break the UI). */
export function flushUiState(): void {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  void saveUiState(cache as Record<string, unknown>).catch(() => {});
}

/** One-time pickup of prefs saved by pre-ui.json builds, then clear them so
 *  localStorage stops shadowing the file. */
function migrateFromLocalStorage(): UiState {
  const state: UiState = {};
  try {
    for (const [key, legacy] of Object.entries(LEGACY_KEYS) as [keyof UiState, string][]) {
      const raw = localStorage.getItem(legacy);
      if (raw === null) continue;
      state[key] = (key === "docks" ? JSON.parse(raw) : raw) as never;
      localStorage.removeItem(legacy);
    }
    if (Object.keys(state).length > 0) void saveUiState(state as Record<string, unknown>).catch(() => {});
  } catch {
    /* a blocked localStorage or bad JSON just means defaults */
  }
  return state;
}

/** Test hook: reset the cache (and any pending save) to a clean slate. */
export function resetUiState(state: UiState = {}): void {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  cache = state;
}
