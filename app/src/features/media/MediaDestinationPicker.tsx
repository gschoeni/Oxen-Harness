// Where the project's generations go on the hub, shown in the Gallery's
// header and switched from there (or with `/media-repo`). The popover lists
// what the API key can write to — its own namespace, organizations it may
// create repos in, repos shared with it — as namespace chips over a
// filterable repository list. A typed `namespace/name`, or a new name in a
// namespace that allows it, is offered as its own row; the backend checks
// it against the hub before saving, and its refusal shows in the menu. A
// switch reaches the open chats' next generation.

import { useEffect, useMemo, useState } from "react";
import { ChevronDown, CloudUpload, ExternalLink, Plus, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui";
import { Menu, MenuHead, MenuItem, MenuSep, useMenuState } from "../../components/ui/Menu";
import { listMediaNamespaces, openExternal } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { MediaDestination, MediaNamespace } from "../../lib/types";

const SOURCE_LABEL: Record<MediaDestination["source"], string> = {
  project: "picked for this project",
  project_remote: "the project's repository",
  global: "the default in Settings → Media",
  hub_default: "the hub's default",
};

const NAME = /^[A-Za-z0-9._-]+$/;

/** `namespace/name`, normalized, when `text` is one. */
export function asRepo(text: string): string | null {
  const clean = text.trim().replace(/^\/+|\/+$/g, "");
  const [ns, name, ...rest] = clean.split("/");
  return rest.length === 0 && ns && name && NAME.test(ns) && NAME.test(name) ? `${ns}/${name}` : null;
}

function kindLabel(ns: MediaNamespace): string {
  if (ns.kind === "user") return "you";
  if (ns.kind === "org") return ns.role ?? "org";
  return "shared";
}

export function MediaDestinationPicker({ root }: { root: string }) {
  const destination = useStore((s) => s.mediaDestinations[root]);
  const refresh = useStore((s) => s.refreshMediaDestination);
  const choose = useStore((s) => s.chooseMediaDestination);
  const pick = useStore((s) => s.mediaDestinationPick);
  const clearPick = useStore((s) => s.clearMediaDestinationPick);
  const { open, setOpen, ref } = useMenuState();
  const [namespaces, setNamespaces] = useState<MediaNamespace[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refresh(root);
  }, [root, refresh]);

  // `/media-repo` asks for the menu (possibly before this panel mounted).
  useEffect(() => {
    if (!pick) return;
    setOpen(true);
    clearPick();
  }, [pick, setOpen, clearPick]);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setError(null);
    let live = true;
    listMediaNamespaces()
      .then((list) => {
        if (!live) return;
        setNamespaces(list);
        const current = destination?.repo?.split("/")[0]?.toLowerCase();
        const home = list.find((n) => n.name.toLowerCase() === current) ?? list[0];
        setSelected(home?.name ?? null);
      })
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
    // Re-list on each open, not when the destination it changes moves.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const namespace = namespaces?.find((n) => n.name === selected) ?? null;
  const typed = asRepo(query);
  const repos = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!namespace || typed) return [];
    return namespace.repos.filter((r) => r.toLowerCase().includes(needle));
  }, [namespace, query, typed]);
  const newName = query.trim();
  const canCreate =
    !!namespace?.can_create &&
    !typed &&
    NAME.test(newName) &&
    !namespace.repos.some((r) => r.toLowerCase() === newName.toLowerCase());

  async function select(repo: string | null) {
    setBusy(true);
    setError(null);
    try {
      await choose(root, repo);
      setOpen(false);
    } catch (e) {
      // Stay open: the hub's reason (read-only, not a namespace you can
      // create in) is the answer the user needs to pick again.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const current = destination?.repo ?? null;
  const label = current ?? "your playground";

  return (
    <div className="picker media-dest" ref={ref}>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className="picker-btn media-dest-btn"
        onClick={() => setOpen((o) => !o)}
        disabled={busy}
        title={destination ? `Generations go to ${label} (${SOURCE_LABEL[destination.source]})` : "Where generations go on the hub"}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Hub repository: ${label}`}
      >
        <CloudUpload size={13} aria-hidden="true" />
        <span className="picker-label">{label}</span>
        <ChevronDown size={13} className="picker-caret" />
      </Button>

      {open && (
        <Menu className="picker-menu media-dest-menu">
          <MenuHead aside={destination && SOURCE_LABEL[destination.source]}>Generations go to</MenuHead>
          <input
            autoFocus
            className="picker-search"
            type="search"
            placeholder="Filter, or type namespace/name…"
            aria-label="Filter repositories"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !busy) {
                if (typed) void select(typed);
                else if (canCreate && namespace) void select(`${namespace.name}/${newName}`);
                else if (repos.length === 1 && namespace) void select(`${namespace.name}/${repos[0]}`);
              }
            }}
          />
          {namespaces && namespaces.length > 0 && (
            <div className="media-dest-namespaces" role="group" aria-label="Namespace">
              {namespaces.map((n) => (
                <button
                  key={n.name}
                  type="button"
                  className={`gallery-chip ${n.name === selected ? "on" : ""}`}
                  aria-pressed={n.name === selected}
                  title={`${n.name} · ${kindLabel(n)}`}
                  onClick={() => setSelected(n.name)}
                >
                  {n.name}
                  <span className="media-dest-kind">{kindLabel(n)}</span>
                </button>
              ))}
            </div>
          )}
          <div className="picker-scroll">
            {!namespaces && !error && <div className="menu-empty">Loading repositories…</div>}
            {typed && (
              <MenuItem
                checkSlot={<Plus size={15} className="menu-check" />}
                name={`Use ${typed}`}
                hint="checked with the hub"
                disabled={busy}
                onSelect={() => void select(typed)}
              />
            )}
            {namespace &&
              repos.map((repo) => {
                const full = `${namespace.name}/${repo}`;
                return (
                  <MenuItem
                    key={repo}
                    active={full.toLowerCase() === current?.toLowerCase()}
                    name={repo}
                    disabled={busy}
                    onSelect={() => void select(full)}
                  />
                );
              })}
            {canCreate && namespace && (
              <MenuItem
                checkSlot={<Plus size={15} className="menu-check" />}
                name={`Create ${namespace.name}/${newName}`}
                hint="made on the first generation"
                disabled={busy}
                onSelect={() => void select(`${namespace.name}/${newName}`)}
              />
            )}
            {namespace && !typed && repos.length === 0 && !canCreate && (
              <div className="menu-empty">
                {namespace.can_create ? "No repositories yet — type a name to create one." : "No matching repositories."}
              </div>
            )}
          </div>
          {error && (
            <div className="menu-empty menu-error" role="alert">
              {error}
            </div>
          )}
          {(destination?.source === "project" || destination?.web_url) && <MenuSep />}
          {destination?.source === "project" && (
            <MenuItem
              manage
              checkSlot={<RotateCcw size={15} className="menu-check" />}
              name="Use the default"
              hint="project repo, Settings, or your playground"
              disabled={busy}
              onSelect={() => void select(null)}
            />
          )}
          {destination?.web_url && (
            <MenuItem
              manage
              checkSlot={<ExternalLink size={15} className="menu-check" />}
              name="Open on the hub"
              onSelect={() => {
                setOpen(false);
                if (destination.web_url) void openExternal(destination.web_url);
              }}
            />
          )}
        </Menu>
      )}
    </div>
  );
}
