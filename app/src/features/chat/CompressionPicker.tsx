import { useState } from "react";
import { ChevronDown, Settings2, Shrink } from "lucide-react";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { useStore } from "../../lib/store";
import type { CompressionMode } from "../../lib/types";

/** The three modes with the copy shown in the menu. */
const MODES: { value: CompressionMode; name: string; hint: string }[] = [
  { value: "off", name: "Off", hint: "send requests exactly as recorded" },
  { value: "audit", name: "Audit", hint: "measure would-be savings, change nothing" },
  { value: "on", name: "On", hint: "compress stale tool output (originals retrievable)" },
];

/** A compact dropdown in the composer, next to the model picker, for switching
 *  context compression. Applies to the live chat immediately (and persists as
 *  the default for new ones), so it can be set before the first message.
 *  Disabled mid-turn so a switch never contends with a running agent. */
export function CompressionPicker({ disabled }: { disabled: boolean }) {
  // The live agent's actual mode; falls back to "off" before a session exists.
  const mode = useStore((s) => s.session?.compression_mode ?? "off");
  const changeCompressionMode = useStore((s) => s.changeCompressionMode);
  const openSettings = useStore((s) => s.openSettings);

  const { open, setOpen, ref } = useMenuState();
  const [busy, setBusy] = useState(false);

  async function pick(next: CompressionMode) {
    setOpen(false);
    if (next === mode) return;
    setBusy(true);
    try {
      await changeCompressionMode(next);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="picker" ref={ref}>
      <button
        type="button"
        className="picker-btn"
        onClick={() => setOpen((o) => !o)}
        disabled={disabled || busy}
        title={
          disabled
            ? "Finish the current turn to switch compression"
            : "Context compression: shrink stale tool output before each request"
        }
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <Shrink size={13} />
        <span className="picker-label">Compression {mode}</span>
        <ChevronDown size={13} className="picker-caret" />
      </button>

      {open && (
        <Menu className="picker-menu">
          <MenuHead>Context compression</MenuHead>
          {MODES.map((m) => (
            <MenuItem
              key={m.value}
              active={m.value === mode}
              name={m.name}
              hint={m.hint}
              onSelect={() => pick(m.value)}
            />
          ))}
          <MenuSep />
          <MenuItem
            manage
            checkSlot={<Settings2 size={15} className="menu-check" />}
            name="Compression settings…"
            onSelect={() => {
              setOpen(false);
              openSettings("compression");
            }}
          />
        </Menu>
      )}
    </div>
  );
}
