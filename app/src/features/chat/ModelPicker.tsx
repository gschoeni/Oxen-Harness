import { useEffect, useReducer, useState } from "react";
import { ChevronDown, Cloud, Cpu, Download, Loader } from "lucide-react";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { installedLocalModels, searchOxenModels } from "../../lib/ipc";
import { ratesById } from "../../lib/rates";
import { useStore } from "../../lib/store";
import type { ModelRef, StartupModelChoice } from "../../lib/types";

/** A compact model dropdown. In the chat composer it switches the active
 *  session; with `onStartupChoice` it only stages a model for a future chat. */
export function ModelPicker({
  disabled,
  startupChoice,
  onStartupChoice,
}: {
  disabled: boolean;
  startupChoice?: StartupModelChoice | null;
  onStartupChoice?: (choice: StartupModelChoice) => void;
}) {
  const sessionModel = useStore((s) => s.session?.model);
  const model = startupChoice?.id ?? sessionModel;
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
  // Per-million price labels from the endpoint catalog, keyed by model id.
  // Kept across opens so rows show a (possibly stale) rate instantly while a
  // refresh is in flight; a failed fetch just means no tags.
  const [rates, setRates] = useState<Map<string, string>>(new Map());

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
  const label = startupChoice?.label ?? current?.name ?? model ?? "Model";

  // What the button reads while working: a phased message for a local-model
  // start (its server takes a moment — and several seconds on a cold first run),
  // or a plain "Switching…" for an in-place cloud swap.
  const choosingStartupModel = !!onStartupChoice;
  const switching = choosingStartupModel ? busy : busy || !!localSwitch;
  const elapsed = localSwitch ? Math.max(0, Math.round((Date.now() - localSwitch.startedAt) / 1000)) : 0;
  const statusLabel = !choosingStartupModel && localSwitch
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

  // Refresh the catalog, installed local models, and price tags when the menu
  // opens.
  useEffect(() => {
    if (!open) return;
    loadCloudModels();
    installedLocalModels()
      .then((v) => setLocalModels(v.models))
      .catch(() => setLocalModels([]));
    searchOxenModels("")
      .then((hits) => setRates(ratesById(hits)))
      .catch(() => {});
  }, [open, loadCloudModels]);

  async function pickCloud(id: string, name: string) {
    setOpen(false);
    if (id === model) return;
    if (onStartupChoice) {
      onStartupChoice({ id, label: name, local: false });
      return;
    }
    setBusy(true);
    try {
      await changeModel(id);
    } finally {
      setBusy(false);
    }
  }

  async function pickLocal(local: ModelRef) {
    setOpen(false);
    if (local.id === model) return;
    if (onStartupChoice) {
      onStartupChoice({ id: local.id, label: local.display, local: true });
      return;
    }
    setBusy(true);
    try {
      await switchToLocalModel(local.id);
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
            : !choosingStartupModel && localSwitch
              ? "Starting the local model…"
              : choosingStartupModel
                ? "Choose the starting model"
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

      {!choosingStartupModel && localSwitch && (
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
            {cloudModels.length === 0 && (
              <MenuItem
                manage
                name="None yet — add one from the catalog…"
                onSelect={() => {
                  setOpen(false);
                  openSettings("cloud-models");
                }}
              />
            )}
            {cloudModels.map((m) => (
              <MenuItem
                key={m.id}
                active={m.id === model}
                name={m.name}
                hint={
                  rates.has(m.id) ? (
                    <span className="menu-hint-stack">
                      <span>{m.id}</span>
                      <span className="menu-rate">{rates.get(m.id)}</span>
                    </span>
                  ) : (
                    m.id
                  )
                }
                onSelect={() => pickCloud(m.id, m.name)}
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
                    hint={<span className="menu-rate">free</span>}
                    onSelect={() => pickLocal(m)}
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
