// The right-click menu on a tab, pinned where the pointer was. Outside click,
// Escape, or picking an item dismisses it; the caller acts on the pick.

import { ArrowRightToLine, Copy, Pencil, SquareX, Trash2, X } from "lucide-react";
import { ContextMenu, MenuItem, MenuSep, type MenuAt } from "../../components/ui/Menu";

export type { MenuAt };
export type TabAction = "rename" | "close" | "close-others" | "close-right" | "copy-id" | "delete";

export function TabMenu({
  at,
  onClose,
  onPick,
}: {
  at: MenuAt;
  onClose: () => void;
  onPick: (action: TabAction) => void;
}) {
  return (
    <ContextMenu at={at} onClose={onClose} label="Tab actions">
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
        className="danger"
        name="Delete chat…"
        onSelect={() => onPick("delete")}
      />
    </ContextMenu>
  );
}
