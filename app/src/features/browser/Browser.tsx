// The link-browser pane: a web page the user clicked in the chat, shown in the
// right dock instead of hijacking the main webview (which would replace the
// whole app UI with the page, full-window, with no way back — see lib/links.ts).
//
// Same architecture as the live-preview pane: the page renders in a NATIVE
// child webview owned by the Rust side (src-tauri/src/browser.rs); this
// component renders the frame around it and keeps the native view glued to the
// placeholder div, detaching whenever an overlay must appear above it (native
// views always paint over the DOM).

import { useEffect, useRef, type PointerEvent } from "react";
import { ExternalLink, Globe2, RotateCw, X } from "lucide-react";
import { useStore } from "../../lib/store";
import { browserAttach, browserClose, browserDetach, browserReload, openExternal } from "../../lib/ipc";
import { useOverlayOpen } from "../preview/useOverlayOpen";
import "../preview/preview.css";

export function Browser({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const url = useStore((s) => s.browserUrl);
  const closeBrowser = useStore((s) => s.closeBrowser);
  const overlayOpen = useOverlayOpen();
  const frameRef = useRef<HTMLDivElement>(null);
  const showNative = !!url && !overlayOpen;

  // Keep the native webview glued to the placeholder while it's visible —
  // the same measure/observe dance as the preview pane (see its comments for
  // why the rect is re-checked on an animation frame and deduped).
  useEffect(() => {
    if (!showNative || !url) {
      browserDetach().catch(() => {});
      return;
    }
    const el = frameRef.current;
    if (!el) return;

    let raf = 0;
    let last = "";
    const measure = () => {
      const r = el.getBoundingClientRect();
      const bounds = {
        x: Math.round(r.x),
        y: Math.round(r.y),
        width: Math.round(r.width),
        height: Math.round(r.height),
      };
      const key = `${bounds.x},${bounds.y},${bounds.width},${bounds.height}`;
      if (key === last) return;
      last = key;
      browserAttach(url, bounds).catch(() => {});
    };
    const schedule = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(measure);
    };
    measure();

    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    // The placeholder's x also moves when the column is dragged or the
    // sidebar changes, without its own size changing.
    const pane = el.closest(".browser-pane");
    if (pane) ro.observe(pane);
    window.addEventListener("resize", schedule);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", schedule);
      cancelAnimationFrame(raf);
      browserDetach().catch(() => {});
    };
  }, [showNative, url]);

  if (!url) return null;

  return (
    <aside className="canvas preview-pane browser-pane">
      {onResizeStart && (
        <div
          className="canvas-resizer"
          onPointerDown={onResizeStart}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize browser"
        />
      )}
      <header className="canvas-head preview-head">
        <div className="preview-location" title={url}>
          <Globe2 className="preview-location-icon" size={14} aria-hidden="true" />
          <span className="preview-url">{url}</span>
        </div>
        <div className="preview-actions">
          <button
            className="icon-btn sm"
            aria-label="Reload page"
            title="Reload"
            onClick={() => browserReload().catch(() => {})}
          >
            <RotateCw size={14} />
          </button>
          <button
            className="icon-btn sm"
            aria-label="Open in browser"
            title="Open in browser"
            onClick={() => openExternal().catch(() => {})}
          >
            <ExternalLink size={14} />
          </button>
          <button
            className="icon-btn sm"
            aria-label="Close browser pane"
            title="Close"
            onClick={() => {
              browserClose().catch(() => {});
              closeBrowser();
            }}
          >
            <X size={15} />
          </button>
        </div>
      </header>
      <div className="preview-body">
        <div className="preview-stage">
          <div ref={frameRef} className="preview-frame">
            {overlayOpen && <p className="preview-hint">Page hidden while a panel is open</p>}
          </div>
        </div>
      </div>
    </aside>
  );
}
