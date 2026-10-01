// Getting to the game while an agent runs. The cabinet lives in the right-hand
// work panel as the "study" view; the titlebar button, the composer's button,
// the shortcut, and the nudge all go through here so they can't disagree
// about what opening or hiding it means.

import { STUDY_VIEW, useStore } from "../../lib/store";

/** How the shortcut reads in tooltips and hints. */
export const STUDY_SHORTCUT = "⌘J";

type Layout = Pick<ReturnType<typeof useStore.getState>, "session" | "rightTab" | "dockCollapsed">;

const showing = (s: Layout) =>
  !!s.session && s.rightTab[s.session.session_id] === STUDY_VIEW && !s.dockCollapsed.right;

/** Whether the game is on screen in the work panel for the chat shown. */
export function useStudyViewShowing(): boolean {
  return useStore(showing);
}

/** Bring the work panel to the game on whichever cabinet is selected. */
export function showStudyView() {
  useStore.getState().setRightTab(STUDY_VIEW);
}

/** Show the game, or fold the work panel away if the game is what it shows. */
export function toggleStudyView() {
  const s = useStore.getState();
  if (showing(s)) s.setDockCollapsed("right", true);
  else showStudyView();
}

/** Open the work panel on the study cabinet (it starts itself there). */
export function openStudyView() {
  useStore.getState().setHeroGame("study");
  showStudyView();
}
