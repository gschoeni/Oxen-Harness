// One side's dock column: the active dock's content, a drag handle, and (when
// more than one dock on that side has content) a tab strip to switch between
// them. Collapsed, the column becomes a thin rail of icons — click one to
// bring it back.
//
// Nothing here knows what a preview or a canvas is: it renders whatever the
// registry (`docks.tsx`) says is available on this side.

import { useCallback, useEffect, type PointerEvent } from "react";
import { PanelLeftOpen, PanelRightOpen } from "lucide-react";
import { useStore } from "../../lib/store";
import { DockToggle } from "./DockToggle";
import { useAvailableDocks, type DockSide, type DockSpec } from "./docks";
import "./docks.css";

/** Smallest the chat column may be squeezed to by a dock drag. */
const CHAT_MIN = 380;
/** Width of a collapsed column's icon rail. */
export const RAIL_W = 52;

/** The dock the user is looking at on `side` (falls back to the first with
 *  content, so a side always shows *something* when it's open). */
export function useActiveDock(side: DockSide): DockSpec | undefined {
  const available = useAvailableDocks(side);
  const activeId = useStore((s) => {
    const session = s.session?.session_id;
    // Only the right side is per-chat today (preview ⇄ canvas). A left-side
    // tab would key off the same map when one exists.
    return session && side === "right" ? s.rightTab[session] : undefined;
  });
  return available.find((d) => d.id === activeId) ?? available[0];
}

export function DockColumn({ side }: { side: DockSide }) {
  const available = useAvailableDocks(side);
  const active = useActiveDock(side);
  const collapsed = useStore((s) => !!s.dockCollapsed[side]);
  const setDockWidth = useStore((s) => s.setDockWidth);
  const setDockCollapsed = useStore((s) => s.setDockCollapsed);
  const setRightTab = useStore((s) => s.setRightTab);

  const minWidth = active?.minWidth ?? 240;

  // Drag the divider between the chat and this dock. The dock grows toward the
  // chat, clamped so both stay usable.
  const beginResize = useCallback(
    (e: PointerEvent) => {
      e.preventDefault();
      document.body.classList.add("dock-resizing");
      const otherSide: DockSide = side === "left" ? "right" : "left";
      const other = useStore.getState();
      const otherWidth = other.dockCollapsed[otherSide]
        ? RAIL_W
        : (other.dockWidths[otherSide] ?? 0);

      const move = (ev: globalThis.PointerEvent) => {
        const max = window.innerWidth - otherWidth - CHAT_MIN;
        const raw = side === "left" ? ev.clientX : window.innerWidth - ev.clientX;
        setDockWidth(side, Math.max(minWidth, Math.min(raw, Math.max(minWidth, max))));
      };
      const up = () => {
        document.body.classList.remove("dock-resizing");
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    },
    [side, minWidth, setDockWidth],
  );

  if (!available.length) return null;

  // Whatever mark this side wants to keep visible when collapsed (the app's
  // logo, for the chat list) — so the column's top holds its place and the
  // layout doesn't lurch when you collapse it.
  const railHeader = available.find((d) => d.railHeader)?.railHeader;

  if (collapsed) {
    return (
      <nav className={`dock-rail ${side}`} aria-label={`Collapsed ${side} panel`}>
        {/* Keep the window draggable by its title bar even when collapsed. */}
        <div className="dock-rail-titlebar" data-tauri-drag-region />
        {railHeader && <div className="dock-rail-brand">{railHeader()}</div>}
        <button
          className="dock-rail-btn"
          onClick={() => setDockCollapsed(side, false)}
          title={`Expand (${side === "left" ? "⌘B" : "⌘⌥B"})`}
          aria-label={`Expand ${side} panel`}
        >
          {side === "left" ? <PanelLeftOpen size={17} /> : <PanelRightOpen size={17} />}
        </button>
        <div className="dock-rail-sep" />
        {available.map((dock) => (
          <button
            key={dock.id}
            className="dock-rail-btn"
            title={dock.title}
            aria-label={`Open ${dock.title}`}
            onClick={() => {
              setDockCollapsed(side, false);
              if (side === "right") setRightTab(dock.id as "preview" | "canvas");
            }}
          >
            {dock.icon}
          </button>
        ))}
      </nav>
    );
  }

  return (
    <div className={`dock-column ${side}`}>
      {available.length > 1 && (
        <div className="dock-tabs" role="tablist" aria-label={`${side} panels`}>
          {available.map((dock) => (
            <button
              key={dock.id}
              role="tab"
              aria-selected={dock.id === active?.id}
              className={`dock-tab${dock.id === active?.id ? " active" : ""}`}
              onClick={() => side === "right" && setRightTab(dock.id as "preview" | "canvas")}
            >
              {dock.icon}
              <span>{dock.title}</span>
            </button>
          ))}
        </div>
      )}
      <div className="dock-body">{active?.render({ onResizeStart: beginResize })}</div>
      {/* The left dock renders its own toggle inside the sidebar header (next
          to the project name); the right column has no shared header of its
          own, so the toggle is pinned into its panel's header band. */}
      {side === "right" && <DockToggle side="right" />}
    </div>
  );
}

/** ⌘B collapses/expands the left column, ⌘⌥B the right — the shortcuts every
 *  editor uses for exactly this. */
export function useDockShortcuts() {
  const toggleDock = useStore((s) => s.toggleDock);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== "b" || !(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      toggleDock(e.altKey ? "right" : "left");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleDock]);
}
