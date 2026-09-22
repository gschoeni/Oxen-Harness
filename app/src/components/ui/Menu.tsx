// Dropdown-menu primitives shared by the composer pickers (model, compression)
// and any other popover list. Token-driven, no feature logic: the caller owns
// the trigger button and the open state (via useMenuState), this file owns the
// popover chrome, rows, and keyboard behavior.
import { useEffect, useId, useRef, useState } from "react";
import { Check } from "lucide-react";
import type {
  ButtonHTMLAttributes,
  HTMLAttributes,
  ReactNode,
  RefObject,
} from "react";

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
      if (ref.current && !ref.current.contains(e.target as Node))
        setOpen(false);
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
export function Menu({
  className = "",
  children,
  onKeyDown: handleKeyDown,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  const ref = useRef<HTMLDivElement>(null);

  function onKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    handleKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = Array.from(
      ref.current?.querySelectorAll<HTMLButtonElement>(
        ".menu-item:not(:disabled)",
      ) ?? [],
    );
    if (items.length === 0) return;
    e.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const delta = e.key === "ArrowDown" ? 1 : -1;
    const next =
      current < 0 ? 0 : (current + delta + items.length) % items.length;
    items[next].focus();
  }

  return (
    <div
      {...props}
      className={`menu ${className}`}
      role="listbox"
      ref={ref}
      onKeyDown={onKeyDown}
    >
      {children}
    </div>
  );
}

/** A section eyebrow. `aside` is a right-aligned legend for the rows below
 *  (the unit their figures share), so no row has to repeat it. */
export function MenuHead({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="menu-head">
      <span>{children}</span>
      {aside && <span className="menu-head-aside">{aside}</span>}
    </div>
  );
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
  description,
  hint,
  onSelect,
  className = "",
  ...props
}: {
  active?: boolean;
  manage?: boolean;
  checkSlot?: ReactNode;
  icon?: ReactNode;
  name: ReactNode;
  description?: ReactNode;
  hint?: ReactNode;
  onSelect: () => void;
} & Omit<ButtonHTMLAttributes<HTMLButtonElement>, "name" | "onSelect">) {
  const descriptionId = useId();
  return (
    <button
      {...props}
      type="button"
      className={[
        "menu-item",
        active ? "active" : "",
        manage ? "manage" : "",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={onSelect}
      role="option"
      aria-selected={active}
      aria-describedby={description ? descriptionId : props["aria-describedby"]}
    >
      {checkSlot ?? <Check size={15} className="menu-check" />}
      {icon}
      {description ? (
        <span className="menu-copy">
          <span className="menu-name">{name}</span>
          <span className="menu-description" id={descriptionId}>
            {description}
          </span>
        </span>
      ) : (
        <span className="menu-name">{name}</span>
      )}
      {hint !== undefined && <span className="menu-hint">{hint}</span>}
    </button>
  );
}
