// Dropdown-menu primitives shared by the composer pickers (model, compression)
// and any other popover list. Token-driven, no feature logic: the caller owns
// the trigger button and the open state (via useMenuState), this file owns the
// popover chrome, rows, and keyboard behavior.
import { useEffect, useRef, useState } from "react";
import { Check } from "lucide-react";
import type { ReactNode, RefObject } from "react";

/** Open/close state with the standard dismissal behavior: outside click and
 *  Escape both close. Attach `ref` to the wrapper containing trigger + menu. */
export function useMenuState(): {
  open: boolean;
  setOpen: (v: boolean | ((o: boolean) => boolean)) => void;
  ref: RefObject<HTMLDivElement | null>;
} {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return { open, setOpen, ref };
}

/** The popover surface. Arrow keys move focus between the menu's items (a
 *  roving listbox), so the pickers are keyboard-navigable, not click-only. */
export function Menu({ className = "", children }: { className?: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = Array.from(
      ref.current?.querySelectorAll<HTMLButtonElement>(".menu-item:not(:disabled)") ?? [],
    );
    if (items.length === 0) return;
    e.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const delta = e.key === "ArrowDown" ? 1 : -1;
    const next = current < 0 ? 0 : (current + delta + items.length) % items.length;
    items[next].focus();
  }

  return (
    <div className={`menu ${className}`} role="listbox" ref={ref} onKeyDown={onKeyDown}>
      {children}
    </div>
  );
}

export function MenuHead({ children }: { children: ReactNode }) {
  return <div className="menu-head">{children}</div>;
}

export function MenuSep() {
  return <div className="menu-sep" />;
}

/** One selectable row: a check that appears when active (replaceable via
 *  `checkSlot` — footer actions show their own glyph there), an optional extra
 *  leading `icon`, the name, and a right-aligned hint. `manage` styles footer
 *  actions ("Add a model…") that navigate instead of selecting. */
export function MenuItem({
  active = false,
  manage = false,
  checkSlot,
  icon,
  name,
  hint,
  onSelect,
}: {
  active?: boolean;
  manage?: boolean;
  checkSlot?: ReactNode;
  icon?: ReactNode;
  name: ReactNode;
  hint?: ReactNode;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className={["menu-item", active ? "active" : "", manage ? "manage" : ""]
        .filter(Boolean)
        .join(" ")}
      onClick={onSelect}
      role="option"
      aria-selected={active}
    >
      {checkSlot ?? <Check size={15} className="menu-check" />}
      {icon}
      <span className="menu-name">{name}</span>
      {hint !== undefined && <span className="menu-hint">{hint}</span>}
    </button>
  );
}
