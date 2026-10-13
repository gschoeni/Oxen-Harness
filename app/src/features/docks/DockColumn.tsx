// One side's dock column: the active dock's content, a drag handle, and (when
// more than one dock on that side has content) a tab strip to switch between
// them. A column the user hid (the title bar's panel switch) renders nothing;
// only the layout solver's squeeze (a window too narrow for it) folds it to a
// thin rail — the panel-open glyph alone when the side has one dock (the
// usual case: Files, the work view), plus one button per dock when it has
// several.
//
// Nothing here knows what a preview or a canvas is: it renders whatever the
// registry (`docks.tsx`) says is available on this side.

import { useCallback, useEffect, type PointerEvent } from "react";
import { PanelLeftOpen, PanelRightOpen } from "lucide-react";
import { useStore, type RightTabId } from "../../lib/store";
import { ColumnNav, setPanelShown, togglePanel, useAvailableDocks, useDockCollapsed, type DockSide, type DockSpec } from "./docks";
import { RAIL_W } from "./layout";
import "./docks.css";

export { RAIL_W };

/** Smallest the chat column may be squeezed to by a dock drag. */
const CHAT_MIN = 380;

/** The dock the user is looking at on `side` (falls back to the first with
 *  content, so a side always shows *something* when it's open). */
export function useActiveDock(side: DockSide): DockSpec | undefined {
  const available = useAvailableDocks(side);
  const activeId = useStore((s) => {
    if (side === "left") return s.leftTab ?? undefined;
    // The right side is per-chat (preview ⇄ canvas ⇄ editor follow the session);
    // the left is app-wide (the file tree follows the workspace, not the chat).
    const session = s.session?.session_id;
    return session ? s.rightTab[session] : undefined;
  });
  return available.find((d) => d.id === activeId) ?? available[0];
}

export function DockColumn({
  side,
  forceRail = false,
}: {
  side: DockSide;
  /** The layout solver squeezed this column to its rail (window too small). */
  forceRail?: boolean;
}) {
  const available = useAvailableDocks(side);
  const active = useActiveDock(side);
  const hidden = useDockCollapsed(side);
  const setDockWidth = useStore((s) => s.setDockWidth);
  const setRightTab = useStore((s) => s.setRightTab);
  const setLeftTab = useStore((s) => s.setLeftTab);
  const pickTab = (id: string) =>
    side === "right" ? (id === "workbench" ? undefined : setRightTab(id as RightTabId)) : setLeftTab(id);

  // Out of the squeeze's rail: showing it again makes room (the other dock
  // hides if the window can't fit both beside the chat).
  const expand = () => setPanelShown(side, true);

  const minWidth = active?.minWidth ?? 240;

  // Drag the divider between the chat and this dock. The dock grows toward the
  // chat, clamped so both stay usable.
  const beginResize = useCallback(
    (e: PointerEvent) => {
      e.preventDefault();
      document.body.classList.add("dock-resizing");
      const otherSide: DockSide = side === "left" ? "right" : "left";
      const other = useStore.getState();
      const otherWidth = other.dockCollapsed[otherSide] ? 0 : (other.dockWidths[otherSide] ?? 0);

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

  if (!available.length || hidden) return null;

  if (forceRail) {
    const shortcut = side === "left" ? "⌘B" : "⌘⌥B";
    const open = side === "left" ? <PanelLeftOpen size={15} /> : <PanelRightOpen size={15} />;
    if (available.length === 1) {
      const [dock] = available;
      return (
        <nav className={`dock-rail ${side}`} aria-label={`Collapsed ${side} panel`}>
          <button
            className="dock-rail-btn"
            onClick={expand}
            title={`Open ${dock.title} (${shortcut})`}
            aria-label={`Open ${dock.title}`}
          >
            {open}
          </button>
        </nav>
      );
    }
    return (
      <nav className={`dock-rail ${side}`} aria-label={`Collapsed ${side} panel`}>
        <button
          className="dock-rail-btn"
          onClick={expand}
          title={`Expand (${shortcut})`}
          aria-label={`Expand ${side} panel`}
        >
          {open}
        </button>
        <div className="dock-rail-sep" />
        {available.map((dock) => (
          <button
            key={dock.id}
            className="dock-rail-btn"
            title={dock.title}
            aria-label={`Open ${dock.title}`}
            onClick={() => {
              expand();
              pickTab(dock.id);
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
      <ColumnNav side={side} />
      {available.length > 1 && (
        <div className="dock-tabs" role="tablist" aria-label={`${side} panels`}>
          {available.map((dock) => (
            <button
              key={dock.id}
              role="tab"
              aria-selected={dock.id === active?.id}
              className={`dock-tab${dock.id === active?.id ? " active" : ""}`}
              onClick={() => pickTab(dock.id)}
            >
              {dock.icon}
              <span>{dock.title}</span>
            </button>
          ))}
        </div>
      )}
      <div className="dock-body">{active?.render({ onResizeStart: beginResize })}</div>
    </div>
  );
}

/** ⌘B shows/hides the left column, ⌘⌥B the right — the shortcuts every
 *  editor uses for exactly this. */
export function useDockShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== "b" || !(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      togglePanel(e.altKey ? "right" : "left");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
