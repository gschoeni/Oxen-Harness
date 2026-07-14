// Is anything covering the app right now?
//
// The preview is a NATIVE webview: it paints above the entire DOM, so it must
// be hidden whenever a UI surface should appear "on top" of it. Missing one
// means a dialog renders *behind* the running app and the user can't click it.
//
// Two sources, deliberately:
//  - the store's full-window surfaces (settings, projects, inspector, question);
//  - any modal scrim in the DOM, watched live. Modals are rendered ad hoc by
//    features (the sidebar's delete-chat confirm, editors), and enumerating
//    them here would rot the moment someone adds another — so we watch for the
//    shared `.modal-scrim` class instead, and every modal is safe by default.

import { useEffect, useState } from "react";
import { useStore } from "../../lib/store";

/** CSS selectors that mean "a surface is covering the app". */
const SCRIM_SELECTOR = ".modal-scrim, .settings-overlay";

export function useOverlayOpen(): boolean {
  const storeOverlay = useStore(
    (s) => s.settingsOpen || s.projectsOpen || !!s.inspector || !!s.question,
  );
  const [scrimOpen, setScrimOpen] = useState(false);

  useEffect(() => {
    const check = () => setScrimOpen(!!document.querySelector(SCRIM_SELECTOR));
    check();
    const observer = new MutationObserver(check);
    observer.observe(document.body, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, []);

  return storeOverlay || scrimOpen;
}
