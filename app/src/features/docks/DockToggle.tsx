// The collapse control for a dock column.
//
// It lives in the column's HEADER band — next to the project title on the
// left, in the panel's header row on the right — never on the edge and never
// hover-only: it must be in the same place every time the user looks for it.

import { PanelLeftClose, PanelRightClose } from "lucide-react";
import { useStore } from "../../lib/store";
import type { DockSide } from "./docks";

export function DockToggle({ side }: { side: DockSide }) {
  const setDockCollapsed = useStore((s) => s.setDockCollapsed);
  const shortcut = side === "left" ? "⌘B" : "⌘⌥B";
  return (
    <button
      className={`dock-toggle ${side}`}
      onClick={() => setDockCollapsed(side, true)}
      title={`Collapse panel (${shortcut})`}
      aria-label={`Collapse ${side} panel`}
    >
      {side === "left" ? <PanelLeftClose size={15} /> : <PanelRightClose size={15} />}
    </button>
  );
}
