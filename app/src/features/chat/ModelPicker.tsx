import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { Button } from "../../components/ui";
import { ChevronDown, Cloud, Cpu, Download, Loader, Plus } from "lucide-react";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { addCloudModel, installedLocalModels, searchOxenModels } from "../../lib/ipc";
import { searchChatModels } from "../../lib/modelSearch";
import { catalogById, rateParts } from "../../lib/rates";
import type { RateParts } from "../../lib/rates";
import { useStore } from "../../lib/store";
import type { ModelRef, OxenModelHit, StartupModelChoice } from "../../lib/types";

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
  // The endpoint's catalog, keyed by model id: its per-million price, or null
  // for a listed model the catalog doesn't price by token. Kept across opens
  // so rows show a (possibly stale) rate instantly while a refresh is in
  // flight; a failed fetch just means no tags. Empty until the first fetch.
  const [catalog, setCatalog] = useState<Map<string, RateParts | null>>(new Map());

  // Tick once a second so the local-switch elapsed counter advances in place.
  const [, tick] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    if (!localSwitch) return;
    const t = setInterval(tick, 500);
    return () => clearInterval(t);
  }, [localSwitch]);

  // The full catalog listing, searched when the user types: a hit that isn't
  // saved yet is offered as "add to your list".
  const [hits, setHits] = useState<OxenModelHit[]>([]);
  const [query, setQuery] = useState("");
  const [addError, setAddError] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const priced = [...catalog.values()].some(Boolean);

  // Friendly label for the active model: its catalog name, else the raw id (a
  // local model, or a custom not yet in the catalog).
  // `||`, not `??`: a mid-turn session the backend couldn't read reports its
  // model as "" — that must still read as a button, not a blank.
  const current = cloudModels.find((m) => m.id === model);
  const label = startupChoice?.label || current?.name || model || "Model";

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
    setQuery("");
    setAddError(null);
    loadCloudModels();
    installedLocalModels()
      .then((v) => setLocalModels(v.models))
      .catch(() => setLocalModels([]));
    searchOxenModels("")
      .then((listing) => {
        setCatalog(catalogById(listing));
        setHits(listing);
      })
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

  // Save a catalog model to the user's list, then switch to it.
  async function addAndPick(hit: OxenModelHit) {
    setAddError(null);
    try {
      await addCloudModel(hit.id, hit.name);
      await loadCloudModels();
    } catch (err) {
      setAddError(`Couldn't add ${hit.id}: ${String(err)}`);
      return;
    }
    await pickCloud(hit.id, hit.name);
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

  const needle = query.trim().toLowerCase();
  const matches = (...fields: string[]) =>
    !needle || fields.some((f) => f.toLowerCase().includes(needle));
  // The active model leads each list; the rest run alphabetically by name.
  const byActiveThenName = <T,>(items: T[], id: (t: T) => string, name: (t: T) => string) =>
    [...items].sort(
      (a, b) =>
        Number(id(b) === model) - Number(id(a) === model) ||
        name(a).localeCompare(name(b), undefined, { sensitivity: "base", numeric: true }),
    );
  const shownCloud = byActiveThenName(
    cloudModels.filter((m) => matches(m.id, m.name)),
    (m) => m.id,
    (m) => m.name,
  );
  const shownLocal = byActiveThenName(
    localModels.filter((m) => matches(m.id, m.display)),
    (m) => m.id,
    (m) => m.display,
  );
  // Only while searching: models the endpoint hosts that aren't saved yet.
  const addable = useMemo(() => {
    if (!needle) return [];
    const saved = new Set(cloudModels.map((m) => m.id));
    return searchChatModels(hits, needle)
      .filter((h) => !saved.has(h.id))
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true }))
      .slice(0, 8);
  }, [hits, needle, cloudModels]);
  const nothingFound = !!needle && !shownCloud.length && !shownLocal.length && !addable.length;

  // Enter takes the top result: a saved model first, else the first addable.
  function onSearchKey(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "Enter") return;
    e.preventDefault();
    if (shownCloud[0]) pickCloud(shownCloud[0].id, shownCloud[0].name);
    else if (shownLocal[0]) pickLocal(shownLocal[0]);
    else if (addable[0]) addAndPick(addable[0]);
  }

  return (
    <div className="picker" ref={ref}>
      <Button
        type="button"
        size="sm"
        variant="ghost"
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
      </Button>

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
          <input
            ref={searchRef}
            autoFocus
            className="picker-search"
            type="search"
            placeholder="Search models…"
            aria-label="Search models"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onSearchKey}
          />
          <div className="picker-scroll">
            {(shownCloud.length > 0 || !needle) && (
              <MenuHead aside={priced && "$ per 1M tokens"}>Cloud models</MenuHead>
            )}
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
            {shownCloud.map((m) => {
              const rate = catalog.get(m.id);
              // A saved id the endpoint no longer lists (a retired release,
              // say) can't be priced — and probably won't answer either.
              const unlisted = catalog.size > 0 && !catalog.has(m.id);
              return (
                <MenuItem
                  key={m.id}
                  active={m.id === model}
                  name={m.name}
                  title={m.id}
                  hint={
                    rate ? (
                      <ModelRate rate={rate} />
                    ) : unlisted ? (
                      <span className="menu-rate-note">not in catalog</span>
                    ) : undefined
                  }
                  onSelect={() => pickCloud(m.id, m.name)}
                />
              );
            })}

            {shownLocal.length > 0 && (
              <>
                <MenuHead>Local models</MenuHead>
                {shownLocal.map((m) => (
                  <MenuItem
                    key={m.id}
                    active={m.id === model}
                    icon={<Cpu size={13} className="menu-icon" />}
                    name={m.display}
                    hint={<span className="menu-rate-note">free</span>}
                    onSelect={() => pickLocal(m)}
                  />
                ))}
              </>
            )}

            {addable.length > 0 && (
              <>
                <MenuHead aside={priced && "$ per 1M tokens"}>Available on Oxen</MenuHead>
                {addable.map((h) => {
                  const rate = rateParts(h.pricing);
                  return (
                    <MenuItem
                      key={h.id}
                      checkSlot={<Plus size={15} className="menu-check menu-add" />}
                      name={h.name}
                      title={`Add ${h.id} to your models`}
                      hint={rate ? <ModelRate rate={rate} /> : undefined}
                      onSelect={() => addAndPick(h)}
                    />
                  );
                })}
              </>
            )}
            {nothingFound && <div className="menu-empty">No models match “{query.trim()}”.</div>}
            {addError && <div className="menu-empty menu-error" role="alert">{addError}</div>}
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

/** A model's price beside its name: two right-aligned columns (in, out) so
 *  the figures line up down the list and the eye compares them as a table.
 *  The unit is on the section head, not here. */
function ModelRate({ rate }: { rate: RateParts }) {
  const label = [rate.input && `${rate.input} in`, rate.output && `${rate.output} out`]
    .filter(Boolean)
    .join(", ");
  return (
    <span className="menu-rate" role="img" aria-label={`${label} per million tokens`}>
      <span className="menu-rate-cell">
        {rate.input && (
          <>
            <b>{rate.input}</b>
            <small>in</small>
          </>
        )}
      </span>
      <span className="menu-rate-cell">
        {rate.output && (
          <>
            <b>{rate.output}</b>
            <small>out</small>
          </>
        )}
      </span>
    </span>
  );
}
