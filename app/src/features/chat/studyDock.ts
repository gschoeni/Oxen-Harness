// Getting to the game while an agent runs: the dock's toggle and the "open on
// the study cabinet" action, shared by the titlebar, the composer toolbar,
// the shortcut, and the nudge so they can't disagree about what opening means.

import { useStore } from "../../lib/store";

/** How the shortcut reads in tooltips and hints. */
export const DOCK_SHORTCUT = "⌘J";

/** Show or hide the dock on whichever cabinet is selected. */
export function toggleGameDock() {
  const s = useStore.getState();
  s.setGameDockOpen(!s.gameDockOpen);
}

/** Open the dock on the study cabinet (it starts itself there). */
export function openStudyDock() {
  const s = useStore.getState();
  s.setHeroGame("study");
  s.setGameDockOpen(true);
}
