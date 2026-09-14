// A project directory arriving from the command line (`oxen-harness ui <dir>`),
// optionally with a surface to open once there (`--gallery`, `--settings
// <page>`, `--session <id>`).
//
// Cold starts get the directory for free: the Rust side makes it the active
// project before the webview loads, and hands the surface over once through
// `takeLaunchSurface`. This bridge covers the running-app case too — the
// single-instance guard focuses the window and emits `project://open`, and
// entering the project here roots the next chat in that directory.

import { onProjectOpen, takeLaunchSurface } from "./ipc";
import { useStore } from "./store";
import type { SettingsPage } from "./types";

/** Open the surface a launch asked for, on whatever project is current. */
export async function applyLaunchSurface(surface: string | null | undefined): Promise<void> {
  if (!surface) return;
  const s = useStore.getState();
  if (surface === "gallery") {
    if (!s.session) await s.startNewSession();
    useStore.getState().openGallery();
    return;
  }
  if (surface.startsWith("settings:")) {
    s.openSettings(surface.slice("settings:".length) as SettingsPage);
    return;
  }
  if (surface.startsWith("session:")) {
    await s.resume(surface.slice("session:".length));
  }
}

/** Install once at startup (main.tsx), outside React's lifecycle, like the
 *  agent event bridge. Returns a remover (used by tests; the app never
 *  uninstalls it). */
export function startCliOpenBridge(): () => void {
  const unlisten = onProjectOpen((e) => {
    void useStore
      .getState()
      .enterProject(e.path)
      .then(() => applyLaunchSurface(e.surface));
  });
  // A cold start's surface: the project is already active; open it now.
  takeLaunchSurface()
    .then((surface) => applyLaunchSurface(surface))
    .catch(() => {});
  return () => {
    unlisten.then((off) => off()).catch(() => {});
  };
}
