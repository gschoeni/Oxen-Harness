// The right-click menu on a tab, pinned where the pointer was. Outside click,
// Escape, or picking an item dismisses it; the caller acts on the pick.

import { useEffect, useRef } from "react";
import { ArrowRightToLine, Copy, Pencil, SquareX, Trash2, X } from "lucide-react";
import { Menu, MenuItem, MenuSep } from "../../components/ui/Menu";

export type TabAction = "rename" | "close" | "close-others" | "close-right" | "copy-id" | "delete";

/** Where the menu opened, in viewport coordinates. */
export interface MenuAt {
  x: number;
  y: number;
}

/** The menu's footprint, for keeping it on screen near the window's edges. */
const MENU_W = 240;
const MENU_H = 240;

export function TabMenu({
  at,
  onClose,
  onPick,
}: {
  at: MenuAt;
  onClose: () => void;
  onPick: (action: TabAction) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const left = Math.max(0, Math.min(at.x, window.innerWidth - MENU_W));
  const top = Math.max(0, Math.min(at.y, window.innerHeight - MENU_H));
  return (
    <div className="chat-tab-menu" ref={ref} style={{ left, top }}>
      <Menu>
        <MenuItem checkSlot={<Pencil size={14} />} name="Rename…" onSelect={() => onPick("rename")} />
        <MenuSep />
        <MenuItem checkSlot={<X size={14} />} name="Close" hint="⌘W" onSelect={() => onPick("close")} />
        <MenuItem checkSlot={<SquareX size={14} />} name="Close others" onSelect={() => onPick("close-others")} />
        <MenuItem
          checkSlot={<ArrowRightToLine size={14} />}
          name="Close to the right"
          onSelect={() => onPick("close-right")}
        />
        <MenuSep />
        <MenuItem checkSlot={<Copy size={14} />} name="Copy session id" onSelect={() => onPick("copy-id")} />
        <MenuItem
          checkSlot={<Trash2 size={14} />}
          name={<span className="chat-tab-menu-danger">Delete chat…</span>}
          onSelect={() => onPick("delete")}
        />
      </Menu>
    </div>
  );
}
