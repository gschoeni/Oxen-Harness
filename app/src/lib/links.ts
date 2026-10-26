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
//  - a bare path is a project file (the model links what it made:
//    `[take](generations/…/take.mp4)`) and opens in the Editor/viewer dock.
//    Resolved against the page URL it would be the app's own address;
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
  if (isPathHref(href)) {
    e.preventDefault();
    const path = workspacePath(href, useStore.getState().session?.workspace);
    // Outside the project there's nothing to show it in, and handing a
    // model-written path to the OS would launch whatever it names.
    if (path) useStore.getState().openInViewer([path]);
    return;
  }

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

/** No scheme and not protocol-relative: a filesystem path, not a URL. */
function isPathHref(href: string): boolean {
  return !/^[a-z][a-z0-9+.-]*:/i.test(href) && !href.startsWith("//");
}

/** A path href as a workspace-relative path, or null when it points outside
 *  the workspace. Drops any `?query` / `#fragment` (a `#L12` line anchor). */
function workspacePath(href: string, workspace: string | undefined): string | null {
  let path = href.replace(/[?#].*$/, "");
  try {
    path = decodeURIComponent(path);
  } catch {
    // A stray `%` — the path is literal.
  }
  if (path.startsWith("/")) {
    if (!workspace || !path.startsWith(`${workspace}/`)) return null;
    path = path.slice(workspace.length + 1);
  }
  path = path.replace(/^(\.\/)+/, "");
  if (!path || path.split("/").includes("..")) return null;
  return path;
}
