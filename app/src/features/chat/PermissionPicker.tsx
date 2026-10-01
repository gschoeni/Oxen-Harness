import { useState } from "react";
import { Button } from "../../components/ui";
import { ChevronDown, Settings2, ShieldAlert, ShieldCheck, ShieldOff } from "lucide-react";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { useStore } from "../../lib/store";
import type { PermissionMode } from "../../lib/types";

/** The three modes in escalation order, with the copy shown in the menu. */
const MODES: { value: PermissionMode; name: string; hint: string }[] = [
  { value: "cautious", name: "Cautious", hint: "asks before edits, commits & most commands" },
  { value: "relaxed", name: "Relaxed", hint: "asks before dangerous commands" },
  { value: "bypass", name: "Bypass", hint: "yolo — never asks; circuit breakers still refuse" },
];

const ICON: Record<PermissionMode, typeof ShieldCheck> = {
  cautious: ShieldAlert,
  relaxed: ShieldCheck,
  bypass: ShieldOff,
};

/** A compact dropdown in the composer, next to the model picker, showing the
 *  permission mode this chat is running under and switching it — the desktop
 *  form of the CLI's `--yolo`. The switch is this chat's alone and is never
 *  saved (the default lives in Settings → Permissions), and unlike the other
 *  pickers it works mid-turn: the gate reads the mode per tool call, so a run
 *  that keeps asking can be let loose, or reined in, without stopping it. */
export function PermissionPicker() {
  const session = useStore((s) => s.session?.session_id);
  const mode = useStore((s) => s.session?.permission_mode ?? "relaxed");
  const changePermissionMode = useStore((s) => s.changePermissionMode);
  const openSettings = useStore((s) => s.openSettings);

  const { open, setOpen, ref } = useMenuState();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function pick(next: PermissionMode) {
    if (next === mode) {
      setOpen(false);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await changePermissionMode(next);
      setOpen(false);
    } catch (e) {
      // Keep the menu open on the failure: a chat believed to be in bypass
      // (or out of it) when it isn't is the one thing this must not hide.
      setError(`Couldn't switch permissions: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  const current = MODES.find((m) => m.value === mode) ?? MODES[1];
  const Icon = ICON[current.value];

  return (
    <div className="picker" ref={ref}>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className={`picker-btn ${mode === "bypass" ? "picker-danger" : ""}`}
        onClick={() => setOpen((o) => !o)}
        disabled={!session || busy}
        title={`Permissions for this chat: ${current.hint}`}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <Icon size={13} />
        <span className="picker-label">{current.name}</span>
        <ChevronDown size={13} className="picker-caret" />
      </Button>

      {open && (
        <Menu className="picker-menu">
          <MenuHead>Permissions for this chat</MenuHead>
          {MODES.map((m) => (
            <MenuItem
              key={m.value}
              active={m.value === mode}
              name={m.name}
              hint={m.hint}
              onSelect={() => pick(m.value)}
            />
          ))}
          {error && (
            <div className="menu-empty menu-error" role="alert">
              {error}
            </div>
          )}
          <MenuSep />
          <MenuItem
            manage
            checkSlot={<Settings2 size={15} className="menu-check" />}
            name="Default mode & rules…"
            onSelect={() => {
              setOpen(false);
              openSettings("permissions");
            }}
          />
        </Menu>
      )}
    </div>
  );
}
