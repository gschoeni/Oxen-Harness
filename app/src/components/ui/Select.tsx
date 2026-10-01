import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent, ReactNode } from "react";
import { createPortal } from "react-dom";
import { ChevronDown } from "lucide-react";
import { Menu, MenuHead, MenuItem } from "./Menu";
import "./select.css";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
  icon?: ReactNode;
  /** A short right-aligned figure (a price, a count), also shown on the trigger. */
  hint?: string;
  disabled?: boolean;
}
export interface SelectProps {
  label: string;
  value: string;
  options: readonly SelectOption[];
  onValueChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  /** Put a filter box above the options — for lists too long to scan. */
  searchable?: boolean;
  className?: string;
}

function matches(option: SelectOption, terms: string[]) {
  const text =
    `${option.label} ${option.description ?? ""} ${option.hint ?? ""}`.toLocaleLowerCase();
  return terms.every((term) => text.includes(term));
}

/** Standard single-choice selector. The shared menu also tells native work
 * surfaces to hide while this portal is open, keeping every option reachable. */
export function Select({
  label,
  value,
  options,
  onValueChange,
  placeholder = "Choose…",
  disabled = false,
  searchable = false,
  className = "",
}: SelectProps) {
  const [open, setOpen] = useState(false);
  const [keyboard, setKeyboard] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({
    visibility: "hidden",
  });
  const trigger = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [highlighted, setHighlighted] = useState(value);
  const typed = useRef({ text: "", at: 0 });
  const id = useId();
  const selected = options.find((option) => option.value === value);
  const terms = searchable
    ? query.toLocaleLowerCase().split(/\s+/).filter(Boolean)
    : [];
  const visible = terms.length
    ? options.filter((option) => matches(option, terms))
    : options;
  const enabled = visible.filter((option) => !option.disabled);
  const activeValue = (
    enabled.find((option) => option.value === highlighted) ??
    enabled.find((option) => option.value === value) ??
    enabled[0]
  )?.value;
  const unavailable = disabled || options.every((option) => option.disabled);
  const expanded = open && !unavailable;
  const activeId =
    activeValue === undefined
      ? undefined
      : `${id}-${options.findIndex((option) => option.value === activeValue)}`;

  function items() {
    return Array.from(
      popover.current?.querySelectorAll<HTMLButtonElement>(
        ".menu-item:not(:disabled)",
      ) ?? [],
    );
  }
  function close(restoreFocus = true) {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus();
  }
  function show(fallback: "first" | "last" = "first") {
    const choices = options.filter((option) => !option.disabled);
    setHighlighted(
      choices.find((option) => option.value === value)?.value ??
        (fallback === "last"
          ? choices[choices.length - 1]?.value
          : choices[0]?.value) ??
        "",
    );
    setQuery("");
    typed.current = { text: "", at: 0 };
    setOpen(true);
  }
  useEffect(() => {
    if (unavailable) setOpen(false);
  }, [unavailable]);

  useLayoutEffect(() => {
    if (!expanded) return;
    function place() {
      if (!trigger.current || !popover.current) return;
      const anchor = trigger.current.getBoundingClientRect();
      const edge = 8;
      const gap = 6;
      const width = Math.min(
        Math.max(anchor.width, 360),
        window.innerWidth - edge * 2,
      );
      const height = Math.min(
        popover.current.querySelector(".menu")?.scrollHeight ?? 0,
        640,
      );
      const below = window.innerHeight - anchor.bottom - gap - edge;
      const above = anchor.top - gap - edge;
      const upwards = below < height && above > below;
      const maxHeight = Math.max(0, upwards ? above : below);
      const next: CSSProperties = {
        left: Math.max(
          edge,
          Math.min(anchor.left, window.innerWidth - width - edge),
        ),
        top: upwards ? undefined : anchor.bottom + gap,
        bottom: upwards ? window.innerHeight - anchor.top + gap : undefined,
        width,
        maxHeight: Math.min(640, maxHeight),
      };
      setPosition((previous) =>
        Object.keys(next).every(
          (key) =>
            previous[key as keyof CSSProperties] ===
            next[key as keyof CSSProperties],
        )
          ? previous
          : next,
      );
    }
    place();
    const observer = new ResizeObserver(place);
    if (trigger.current) observer.observe(trigger.current);
    const menu = popover.current?.querySelector(".menu");
    if (menu) observer.observe(menu);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [expanded]);

  useEffect(() => {
    if (expanded) search.current?.focus();
  }, [expanded]);

  useEffect(() => {
    if (expanded)
      items()
        .find((item) => item.dataset.value === activeValue)
        ?.scrollIntoView({ block: "nearest" });
  }, [expanded, activeValue]);

  useEffect(() => {
    if (!expanded) return;
    const outside = (event: Event) => {
      const target = event.target as Node;
      if (
        !trigger.current?.contains(target) &&
        !popover.current?.contains(target)
      )
        setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("focusin", outside);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("focusin", outside);
    };
  }, [expanded]);

  function navigate(event: KeyboardEvent) {
    setKeyboard(true);
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (event.key === "Tab") {
      // Tab follows the normal document order from the combobox, so a search
      // box (portalled to the end of the document) hands focus back first.
      close(searchable);
      return;
    }
    const rows = items();
    if (!rows.length) return;
    const current = rows.findIndex((row) => row.dataset.value === activeValue);
    // In a searchable menu the keys that edit text belong to the search box.
    if (event.key === "Enter" || (event.key === " " && !searchable)) {
      event.preventDefault();
      event.stopPropagation();
      rows[current]?.click();
      return;
    }
    let next: HTMLButtonElement | undefined;
    if (searchable && event.key !== "ArrowDown" && event.key !== "ArrowUp")
      return;
    if (event.key === "Home") next = rows[0];
    else if (event.key === "End") next = rows[rows.length - 1];
    else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const delta = event.key === "ArrowDown" ? 1 : -1;
      next = rows[(current + delta + rows.length) % rows.length];
    } else if (
      event.key.length === 1 &&
      event.key !== " " &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      const now = Date.now();
      const text =
        (now - typed.current.at < 700 ? typed.current.text : "") +
        event.key.toLocaleLowerCase();
      typed.current = { text, at: now };
      const prefix = [...text].every((character) => character === text[0])
        ? text[0]
        : text;
      const start = prefix.length === 1 ? current + 1 : Math.max(current, 0);
      next = rows
        .slice(start)
        .concat(rows.slice(0, start))
        .find((row) =>
          row.dataset.label?.toLocaleLowerCase().startsWith(prefix),
        );
    }
    if (next) {
      event.preventDefault();
      event.stopPropagation();
      setHighlighted(next.dataset.value ?? "");
    }
  }

  return (
    <>
      <button
        type="button"
        ref={trigger}
        className={`select-trigger ${className}`}
        role="combobox"
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={expanded}
        aria-controls={expanded ? id : undefined}
        aria-activedescendant={
          expanded
            ? activeId
            : undefined
        }
        disabled={unavailable}
        title={selected?.label ?? placeholder}
        onClick={() => {
          setKeyboard(false);
          expanded ? close() : show();
        }}
        onKeyDown={(event) => {
          if (expanded) {
            navigate(event);
            return;
          }
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            setKeyboard(true);
            show(event.key === "ArrowUp" ? "last" : "first");
          }
        }}
      >
        {selected?.icon && (
          <span className="select-trigger-icon" aria-hidden="true">
            {selected.icon}
          </span>
        )}
        <span className="select-value">{selected?.label ?? placeholder}</span>
        {selected?.hint && (
          <span className="select-trigger-hint">{selected.hint}</span>
        )}
        <ChevronDown size={14} className="select-chevron" aria-hidden="true" />
      </button>
      {expanded &&
        createPortal(
          <div ref={popover} className="select-popover" style={position}>
            <Menu
              id={id}
              aria-label={label}
              className={`select-menu ${searchable ? "searchable" : ""}`}
              data-keyboard={keyboard}
              onKeyDown={navigate}
            >
              {searchable ? (
                <div className="select-search">
                  <input
                    ref={search}
                    type="search"
                    className="select-search-input"
                    placeholder="Search…"
                    aria-label={`Search ${label}`}
                    aria-controls={id}
                    aria-activedescendant={activeId}
                    spellCheck={false}
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                  />
                </div>
              ) : (
                <MenuHead>{label}</MenuHead>
              )}
              {!visible.length && (
                <div className="select-empty">No matches</div>
              )}
              {visible.map((option) => (
                <MenuItem
                  key={option.value}
                  id={`${id}-${options.indexOf(option)}`}
                  data-highlighted={option.value === activeValue}
                  onPointerDown={(event) => event.preventDefault()}
                  onPointerMove={() => {
                    setKeyboard(false);
                    if (!option.disabled) setHighlighted(option.value);
                  }}
                  data-value={option.value}
                  data-label={option.label}
                  className="select-option"
                  name={option.label}
                  aria-label={option.label}
                  description={option.description}
                  hint={option.hint}
                  icon={
                    option.icon && (
                      <span className="select-option-icon" aria-hidden="true">
                        {option.icon}
                      </span>
                    )
                  }
                  active={option.value === value}
                  disabled={option.disabled}
                  tabIndex={-1}
                  onSelect={() => {
                    close();
                    if (option.value !== value) onValueChange(option.value);
                  }}
                />
              ))}
            </Menu>
          </div>,
          document.body,
        )}
    </>
  );
}
