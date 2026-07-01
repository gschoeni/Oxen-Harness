import { useEffect, useReducer, useRef, useState } from "react";
import { Check, ChevronDown, Cloud, Cpu, Download, Loader, Plus } from "lucide-react";
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

  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [localModels, setLocalModels] = useState<ModelRef[]>([]);
  const ref = useRef<HTMLDivElement>(null);

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

  // Close on an outside click or Escape.
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
    <div className="model-picker" ref={ref}>
      <button
        type="button"
        className="model-picker-btn"
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
          <Loader size={13} className="model-switch-spin" />
        ) : (
          <Cloud size={13} />
        )}
        <span className="model-picker-label">{statusLabel}</span>
        {!switching && <ChevronDown size={13} className="model-picker-caret" />}
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
        <div className="model-menu" role="listbox">
          <div className="model-menu-head">Cloud models</div>
          {cloudModels.map((m) => (
            <button
              key={m.id}
              type="button"
              className={`model-menu-item ${m.id === model ? "active" : ""}`}
              onClick={() => pickCloud(m.id)}
              role="option"
              aria-selected={m.id === model}
            >
              <Check size={14} className="model-menu-check" />
              <span className="model-menu-name">{m.name}</span>
              <span className="model-menu-id">{m.id}</span>
            </button>
          ))}

          {localModels.length > 0 && (
            <>
              <div className="model-menu-head">Local models</div>
              {localModels.map((m) => (
                <button
                  key={m.id}
                  type="button"
                  className={`model-menu-item ${m.id === model ? "active" : ""}`}
                  onClick={() => pickLocal(m.id)}
                  role="option"
                  aria-selected={m.id === model}
                >
                  <Check size={14} className="model-menu-check" />
                  <Cpu size={13} className="model-menu-local-icon" />
                  <span className="model-menu-name">{m.display}</span>
                </button>
              ))}
            </>
          )}

          <div className="model-menu-sep" />
          <button
            type="button"
            className="model-menu-item model-menu-manage"
            onClick={() => {
              setOpen(false);
              openSettings("local-models");
            }}
          >
            <Download size={14} className="model-menu-check" />
            <span className="model-menu-name">Set up a local model…</span>
          </button>
          <button
            type="button"
            className="model-menu-item model-menu-manage"
            onClick={() => {
              setOpen(false);
              openSettings("cloud-models");
            }}
          >
            <Plus size={14} className="model-menu-check" />
            <span className="model-menu-name">Add a cloud model…</span>
          </button>
        </div>
      )}
    </div>
  );
}
