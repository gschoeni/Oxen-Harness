// A project directory arriving from the command line (`oxen-harness ui <dir>`).
//
// Cold starts need no event: the Rust side makes the directory the active
// project before the webview loads. This bridge covers the running-app case —
// the single-instance guard focuses the window and emits `project://open`,
// and entering the project here roots the next chat in that directory.

import { onProjectOpen } from "./ipc";
import { useStore } from "./store";

/** Install once at startup (main.tsx), outside React's lifecycle, like the
 *  agent event bridge. Returns a remover (used by tests; the app never
 *  uninstalls it). */
export function startCliOpenBridge(): () => void {
  const unlisten = onProjectOpen((path) => {
    void useStore.getState().enterProject(path);
  });
  return () => {
    unlisten.then((off) => off()).catch(() => {});
  };
}
