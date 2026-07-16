// Route link clicks away from the main webview.
//
// The main webview IS the app: letting a plain `<a href>` (chat markdown, a
// docs link) navigate it replaces the entire UI with that page, full-window,
// with no way back. So a capture-phase listener intercepts every anchor click
// and routes it:
//
//  - http(s) links open in the link-browser side panel (`openBrowser`) —
//    unless the anchor opted out with `target="_blank"`, which here means
//    "the system browser" (add-credits, API-signup, docs buttons);
//  - any other scheme (mailto:, custom apps) goes to the system handler;
//  - same-page fragments (`href="#…"`) are left alone.
//
// The Rust side backstops this (see `nav-guard` in src-tauri/src/lib.rs): a
// navigation that slips through anyway is cancelled and bounced back as a
// `browser://open` event, which lands in the same `openBrowser`.

import { onBrowserOpen, openExternal } from "./ipc";
import { useStore } from "./store";

/** Install the global link routing. Called once at startup (main.tsx),
 *  outside React's lifecycle, like the agent event bridge. Returns a
 *  remover (used by tests; the app never uninstalls it). */
export function startLinkRouting(): () => void {
  document.addEventListener("click", onDocumentClick, true);
  const unlisten = onBrowserOpen((url) => useStore.getState().openBrowser(url));
  return () => {
    document.removeEventListener("click", onDocumentClick, true);
    unlisten.then((off) => off()).catch(() => {});
  };
}

function onDocumentClick(e: MouseEvent) {
  if (e.defaultPrevented || e.button !== 0) return;
  const anchor = (e.target as Element | null)?.closest?.("a[href]");
  if (!anchor) return;
  const href = anchor.getAttribute("href") ?? "";
  if (href.startsWith("#")) return; // in-page navigation is fine

  let url: URL;
  try {
    url = new URL(href, window.location.href);
  } catch {
    return;
  }
  if (url.protocol === "http:" || url.protocol === "https:") {
    e.preventDefault();
    if (anchor.getAttribute("target") === "_blank") {
      openExternal(url.href).catch(() => {});
    } else {
      useStore.getState().openBrowser(url.href);
    }
  } else if (url.protocol !== window.location.protocol) {
    // mailto:, custom schemes — hand to the OS instead of the webview.
    e.preventDefault();
    openExternal(url.href).catch(() => {});
  }
}
