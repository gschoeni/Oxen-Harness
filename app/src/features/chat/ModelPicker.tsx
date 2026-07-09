import { useEffect, useReducer, useState } from "react";
import { ChevronDown, Cloud, Cpu, Download, Loader } from "lucide-react";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { installedLocalModels } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { ModelRef } from "../../lib/types";

/** A compact dropdown in the composer for switching the chat's model. Cloud
 *  models swap in place (the conversation continues); a local model starts a
 *  fresh chat on it. "Add a model…" jumps to Settings. Disabled mid-turn so a
 *  swap never contends with a running agent. */
export function ModelPicker({ disabled }: { disabled: boolean }) {
  const model = useStore((s) => s.session?.model);
  const cloudModels = useStore((s) => s.cloudModels);
  const loadCloudModels = useStore((s) => s.loadCloudModels);
  const changeModel = useStore((s) => s.changeModel);
  const switchToLocalModel = useStore((s) => s.switchToLocalModel);
  const openSettings = useStore((s) => s.openSettings);
  // Live phase while a local model's server is starting (null when idle).
  const localSwitch = useStore((s) => s.localSwitch);

  const { open, setOpen, ref } = useMenuState();
  const [busy, setBusy] = useState(false);
  const [localModels, setLocalModels] = useState<ModelRef[]>([]);

  // Tick once a second so the local-switch elapsed counter advances in place.
  const [, tick] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    if (!localSwitch) return;
    const t = setInterval(tick, 500);
    return () => clearInterval(t);
  }, [localSwitch]);

  // Friendly label for the active model: its catalog name, else the raw id (a
  // local model, or a custom not yet in the catalog).
  const current = cloudModels.find((m) => m.id === model);
  const label = current?.name ?? model ?? "Model";

  // What the button reads while working: a phased message for a local-model
  // start (its server takes a moment — and several seconds on a cold first run),
  // or a plain "Switching…" for an in-place cloud swap.
  const switching = busy || !!localSwitch;
  const elapsed = localSwitch ? Math.max(0, Math.round((Date.now() - localSwitch.startedAt) / 1000)) : 0;
  const statusLabel = localSwitch
    ? `${
        localSwitch.phase === "loading"
          ? "Loading model"
          : localSwitch.phase === "ready"
            ? "Finishing"
            : "Starting runtime"
      } · ${elapsed}s`
    : busy
      ? "Switching…"
      : label;
  // A cold first run compiles GPU kernels (one-time) — explain a long first wait.
  const firstRunHint = localSwitch?.phase === "starting" && elapsed >= 4;

  // Refresh the catalog + installed local models when the menu opens.
  useEffect(() => {
    if (!open) return;
    loadCloudModels();
    installedLocalModels()
      .then((v) => setLocalModels(v.models))
      .catch(() => setLocalModels([]));
  }, [open, loadCloudModels]);

  async function pickCloud(id: string) {
    setOpen(false);
    if (id === model) return;
    setBusy(true);
    try {
      await changeModel(id);
    } finally {
      setBusy(false);
    }
  }

  async function pickLocal(id: string) {
    setOpen(false);
    if (id === model) return;
    setBusy(true);
    try {
      await switchToLocalModel(id);
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
        disabled={disabled || switching}
        title={
          disabled
            ? "Finish the current turn to switch models"
            : localSwitch
              ? "Starting the local model…"
              : "Switch model"
        }
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        {switching ? (
          <Loader size={13} className="picker-spin" />
        ) : (
          <Cloud size={13} />
        )}
        <span className="picker-label">{statusLabel}</span>
        {!switching && <ChevronDown size={13} className="picker-caret" />}
      </button>

      {localSwitch && (
        <span className="model-switch-inline">
          <span className="model-switch-bar">
            <span />
          </span>
          {firstRunHint && (
            <span className="model-switch-hint">first run · one-time</span>
          )}
        </span>
      )}

      {open && (
        <Menu className="picker-menu">
          {/* Only the model list scrolls — the setup actions below stay pinned
              so they're never pushed off-screen by a long catalog. */}
          <div className="picker-scroll">
            <MenuHead>Cloud models</MenuHead>
            {cloudModels.map((m) => (
              <MenuItem
                key={m.id}
                active={m.id === model}
                name={m.name}
                hint={m.id}
                onSelect={() => pickCloud(m.id)}
              />
            ))}

            {localModels.length > 0 && (
              <>
                <MenuHead>Local models</MenuHead>
                {localModels.map((m) => (
                  <MenuItem
                    key={m.id}
                    active={m.id === model}
                    icon={<Cpu size={13} className="menu-icon" />}
                    name={m.display}
                    onSelect={() => pickLocal(m.id)}
                  />
                ))}
              </>
            )}
          </div>

          <MenuSep />
          <MenuItem
            manage
            checkSlot={<Download size={15} className="menu-check" />}
            name="Set up a local model…"
            onSelect={() => {
              setOpen(false);
              openSettings("local-models");
            }}
          />
          <MenuItem
            manage
            checkSlot={<Cloud size={15} className="menu-check" />}
            name="Configure a cloud model…"
            onSelect={() => {
              setOpen(false);
              openSettings("cloud-models");
            }}
          />
        </Menu>
      )}
    </div>
  );
}
