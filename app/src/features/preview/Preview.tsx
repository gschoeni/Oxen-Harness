// The live-preview pane: shows the current chat's running dev server.
//
// The page itself renders in a NATIVE child webview owned by the Rust side
// (see src-tauri/src/preview.rs) — this component renders the frame around it
// (toolbar, states) and keeps the native webview glued to the placeholder div:
// it measures the div and calls `previewAttach` on mount/resize, and
// `previewDetach` whenever the pane hides or an overlay must appear above
// (native views always paint over the DOM, so overlays and previews can't
// coexist on screen).

import { useEffect, useRef, useState, type PointerEvent } from "react";
import {
  ExternalLink,
  Globe2,
  Monitor,
  RotateCw,
  Smartphone,
  Square,
  Tablet,
  X,
} from "lucide-react";
import { useStore } from "../../lib/store";
import {
  previewAttach,
  previewDetach,
  previewOpenExternal,
  previewReload,
  previewRestart,
  previewStop,
} from "../../lib/ipc";
import { useOverlayOpen } from "./useOverlayOpen";
import "./preview.css";

/** Viewport presets: CSS width of the previewed app (null = fill the pane). */
const VIEWPORTS = [
  { key: "desktop", width: null as number | null, icon: Monitor, label: "Fill width" },
  { key: "tablet", width: 768, icon: Tablet, label: "Tablet (768px)" },
  { key: "phone", width: 390, icon: Smartphone, label: "Phone (390px)" },
];

export function Preview({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const session = useStore((s) => s.session?.session_id);
  const status = useStore((s) => (s.session ? s.previews[s.session.session_id] : undefined));
  // Native webviews paint above the entire DOM: hide the preview while any
  // surface should sit on top of it (a full-window page, or any modal), and
  // re-attach when it goes away.
  const overlayOpen = useOverlayOpen();
  const closePreview = useStore((s) => s.closePreview);
  const pageError = useStore((s) =>
    s.session ? s.previewErrors[s.session.session_id] : undefined,
  );
  const resolvePreviewError = useStore((s) => s.resolvePreviewError);
  const [viewport, setViewport] = useState<string>("desktop");
  // Restart is the pane's primary recovery action: it must show that it's
  // working (a stop+start takes seconds, and repeat clicks would queue full
  // restarts) and must not fail silently.
  const [restarting, setRestarting] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);

  const frameRef = useRef<HTMLDivElement>(null);
  const ready = status?.phase === "ready";
  const showNative = !!session && ready && !overlayOpen;

  // Any new lifecycle news settles the restart button (it started, or it
  // failed and the phase says so).
  useEffect(() => {
    setRestarting(false);
  }, [status?.phase, status?.url]);

  // Keep the native webview glued to the placeholder while it's visible.
  //
  // The placeholder can change POSITION without changing size (dragging the
  // column splitter while a phone/tablet viewport is centered), which no
  // ResizeObserver reports — so the rect is re-checked on an animation frame
  // whenever anything might have moved, and only sent when it actually
  // changed. `viewport` is deliberately NOT a dep: switching presets resizes
  // the placeholder, which the observer already catches, and re-running the
  // effect would detach → blink.
  useEffect(() => {
    if (!showNative || !session) {
      previewDetach().catch(() => {});
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
      previewAttach(session, bounds).catch(() => {});
    };
    const schedule = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(measure);
    };
    measure();

    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    // The pane (and thus the placeholder's x) also moves when the column is
    // dragged or the sidebar changes — observing the pane catches those even
    // when the frame's own size is pinned by a viewport preset.
    const pane = el.closest(".preview-pane");
    if (pane) ro.observe(pane);
    window.addEventListener("resize", schedule);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", schedule);
      cancelAnimationFrame(raf);
      previewDetach().catch(() => {});
    };
    // status.url re-runs the effect after a server restart on a new port.
  }, [showNative, session, status?.url]);

  if (!session || !status) return null;

  return (
    <aside className="canvas preview-pane">
      {onResizeStart && (
        <div
          className="canvas-resizer"
          onPointerDown={onResizeStart}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize preview"
        />
      )}
      <header className="canvas-head preview-head">
        <div className="preview-location" title={status.command}>
          <span className={`preview-status${ready ? " ready" : ""}`} aria-hidden="true" />
          <Globe2 className="preview-location-icon" size={14} aria-hidden="true" />
          <span className="preview-url">{status.url ?? status.name}</span>
          {status.phase === "starting" && <span className="canvas-writing">starting…</span>}
        </div>
        <div className="preview-actions">
          <div className="preview-viewport-group" role="group" aria-label="Preview size">
            {VIEWPORTS.map(({ key, icon: Icon, label }) => (
              <button
                key={key}
                className={`icon-btn sm preview-vp-btn${viewport === key ? " preview-vp-active" : ""}`}
                aria-label={label}
                aria-pressed={viewport === key}
                title={label}
                onClick={() => setViewport(key)}
              >
                <Icon size={14} />
              </button>
            ))}
          </div>
          <span className="preview-sep" aria-hidden="true" />
          <button
            className="icon-btn sm"
            aria-label="Reload preview"
            title="Reload"
            onClick={() => previewReload(session).catch(() => {})}
          >
            <RotateCw size={14} />
          </button>
          <button
            className="icon-btn sm"
            aria-label="Open in browser"
            title="Open in browser"
            onClick={() => previewOpenExternal(session).catch(() => {})}
          >
            <ExternalLink size={14} />
          </button>
          <button
            className="icon-btn sm preview-stop-btn"
            aria-label="Stop server"
            title="Stop server"
            onClick={() => previewStop(session).catch(() => {})}
          >
            <Square size={12} />
          </button>
          <button
            className="icon-btn sm"
            aria-label="Close preview"
            title="Close (server keeps running)"
            onClick={() => {
              previewDetach().catch(() => {});
              closePreview();
            }}
          >
            <X size={15} />
          </button>
        </div>
      </header>
      <div className="preview-body">
        {pageError && (
          <div className="preview-banner" role="alert">
            <span className="preview-banner-text" title={pageError}>
              Something broke in the app: {pageError}
            </span>
            <button className="preview-fix-btn" onClick={() => resolvePreviewError(session, true)}>
              Fix it
            </button>
            <button
              className="icon-btn"
              aria-label="Dismiss error"
              onClick={() => resolvePreviewError(session, false)}
            >
              <X size={13} />
            </button>
          </div>
        )}
        {ready ? (
          <div className="preview-stage">
            <div
              ref={frameRef}
              className="preview-frame"
              style={
                viewportWidth(viewport) != null
                  ? { flex: `0 1 ${viewportWidth(viewport)}px` }
                  : undefined
              }
            >
              {overlayOpen && <p className="preview-hint">Preview hidden while a panel is open</p>}
            </div>
          </div>
        ) : status.phase === "starting" ? (
          <div className="canvas-placeholder">
            <span className="canvas-spinner" />
            <p>Starting {status.name} server…</p>
          </div>
        ) : (
          <div className="preview-error">
            <p className="preview-error-title">
              {status.phase === "error" ? "The server hit a problem" : "Server stopped"}
            </p>
            {status.message && <p className="preview-error-msg">{status.message}</p>}
            <button
              className="preview-fix-btn"
              disabled={restarting}
              onClick={() => {
                setRestarting(true);
                setRestartError(null);
                previewRestart(session).catch((e) => {
                  // Without this the primary recovery button would fail
                  // silently: nothing else reports a refused restart.
                  setRestartError(String(e));
                  setRestarting(false);
                });
              }}
            >
              {restarting ? "Starting…" : "Restart server"}
            </button>
            {restartError && <p className="preview-error-msg">{restartError}</p>}
            <p className="preview-hint">
              Or ask the chat to fix it — for example “the preview stopped, get it running again”.
            </p>
          </div>
        )}
      </div>
    </aside>
  );
}

function viewportWidth(key: string): number | null {
  return VIEWPORTS.find((v) => v.key === key)?.width ?? null;
}
