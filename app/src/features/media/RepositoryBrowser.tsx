// Choose the Oxen repository a project's assets go to, from what the API key
// can write to: its own namespace, organizations it may create repos in, and
// repositories shared with it. Search spans every namespace at once; with no
// query, one namespace's repositories are listed. A typed `namespace/name`,
// or a new name in the selected namespace (which stays visible while
// searching, so it's clear where a new repository would go), is offered as
// its own row. The backend checks the pick against the hub before saving and
// its refusal (read-only, not a namespace you can create in) is shown in
// place.
//
// `columns` lays namespaces beside repositories (Settings, where there is
// room); `compact` stacks a namespace select over the list (the Gallery's
// popover). The search box is a combobox over the result list: arrows move
// the highlighted row (scrolled into view), Enter picks it, Escape closes the
// browser and nothing behind it.

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { ArrowRight, Check, Plus, RefreshCw, Search } from "lucide-react";
import { IconButton } from "../../components/ui";
import { useStore } from "../../lib/store";
import type { MediaNamespace } from "../../lib/types";
import "./media.css";

const NAME = /^[A-Za-z0-9._-]+$/;
/** Search results beyond this are cut; a narrower query finds the rest. */
const MAX_RESULTS = 200;

const GROUPS: { kind: MediaNamespace["kind"]; label: string }[] = [
  { kind: "user", label: "Your account" },
  { kind: "org", label: "Organizations" },
  { kind: "shared", label: "Shared with you" },
];

/** `namespace/name`, normalized, when `text` is one. All-dot halves are
 *  refused, as the backend refuses them. */
export function asRepo(text: string): string | null {
  const clean = text.trim().replace(/^\/+|\/+$/g, "");
  const [ns, name, ...rest] = clean.split("/");
  const ok = (s: string | undefined) => !!s && NAME.test(s) && !/^\.+$/.test(s);
  return rest.length === 0 && ok(ns) && ok(name) ? `${ns}/${name}` : null;
}

type Row =
  | { kind: "repo"; full: string; namespace: string; repo: string }
  | { kind: "use"; full: string }
  | { kind: "create"; full: string };

export function RepositoryBrowser({
  root,
  current,
  layout,
  onDone,
}: {
  root: string;
  /** The repository in effect (`namespace/name`), marked "in use". */
  current: string | null;
  layout: "columns" | "compact";
  /** After a successful switch (or Escape). */
  onDone: () => void;
}) {
  const listed = useStore((s) => s.mediaNamespaces);
  const load = useStore((s) => s.loadMediaNamespaces);
  const choose = useStore((s) => s.chooseMediaDestination);
  const [picked, setPicked] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  // A failed listing (waits for the refresh button) and a refused pick
  // (the hub's reason for that one choice) are kept apart.
  const [listError, setListError] = useState<string | null>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const search = useRef<HTMLInputElement>(null);
  const rowRefs = useRef(new Map<number, HTMLButtonElement>());
  const listId = useId();

  // Two repositories the hub may not list yet but that work: your own
  // `playground` (its default, made on first use) and the one in effect.
  const namespaces = useMemo(() => {
    if (!listed) return null;
    const [currentNs, currentRepo] = current?.split("/") ?? [];
    return listed.map((n) => {
      const extra = [
        ...(n.kind === "user" ? ["playground"] : []),
        ...(currentNs?.toLowerCase() === n.name.toLowerCase() && currentRepo ? [currentRepo] : []),
      ].filter((r) => !n.repos.some((have) => have.toLowerCase() === r.toLowerCase()));
      return extra.length ? { ...n, repos: [...new Set(extra), ...n.repos] } : n;
    });
  }, [listed, current]);

  function list(force: boolean) {
    setLoading(true);
    setListError(null);
    load(force)
      .catch((e) => setListError(String(e)))
      .finally(() => setLoading(false));
  }

  // List on open, and again whenever the cache is dropped (a new key was
  // saved while the browser is open). A failure waits for the refresh button.
  useEffect(() => {
    if (listed === null && !loading && !listError) list(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listed]);

  useEffect(() => {
    search.current?.focus();
  }, []);

  // The namespace on show: the one picked here while it still exists, else
  // the one the repository in effect lives in, else the first.
  const namespace = useMemo(() => {
    if (!namespaces) return null;
    const byName = (name: string | null | undefined) =>
      name ? namespaces.find((n) => n.name.toLowerCase() === name.toLowerCase()) : undefined;
    return byName(picked) ?? byName(current?.split("/")[0]) ?? namespaces[0] ?? null;
  }, [namespaces, picked, current]);

  const needle = query.trim().toLowerCase();
  const typed = asRepo(query);
  const searching = !!needle && !typed;

  const { rows, truncated } = useMemo(() => {
    const out: Row[] = [];
    if (!namespaces) return { rows: out, truncated: false };
    if (typed) return { rows: [{ kind: "use" as const, full: typed }], truncated: false };
    const pool = searching ? namespaces : namespace ? [namespace] : [];
    let cut = false;
    for (const ns of pool) {
      for (const repo of ns.repos) {
        const full = `${ns.name}/${repo}`;
        if (searching && !full.toLowerCase().includes(needle)) continue;
        if (out.length >= MAX_RESULTS) {
          cut = true;
          break;
        }
        out.push({ kind: "repo", full, namespace: ns.name, repo });
      }
      if (cut) break;
    }
    const fresh = query.trim();
    if (
      namespace?.can_create &&
      NAME.test(fresh) &&
      !/^\.+$/.test(fresh) &&
      !namespace.repos.some((r) => r.toLowerCase() === fresh.toLowerCase())
    )
      out.push({ kind: "create", full: `${namespace.name}/${fresh}` });
    return { rows: out, truncated: cut };
  }, [namespaces, namespace, needle, typed, searching, query]);

  useEffect(() => setCursor(0), [needle, namespace?.name]);
  // A refresh can shrink the list under the highlight.
  const active = rows.length ? Math.min(cursor, rows.length - 1) : -1;
  useEffect(() => {
    rowRefs.current.get(active)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  async function pick(full: string) {
    if (busy) return;
    setBusy(true);
    setPickError(null);
    try {
      await choose(root, full);
      onDone();
    } catch (e) {
      setPickError(String(e));
    } finally {
      setBusy(false);
      search.current?.focus();
    }
  }

  function selectNamespace(name: string) {
    setPicked(name);
    setQuery("");
    search.current?.focus();
  }

  // Escape from anywhere inside closes only the browser (Settings closes on
  // a window-level Escape); arrows and Enter drive the list from the box.
  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onDone();
      return;
    }
    if (e.target !== search.current) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setCursor(rows.length ? (active + step + rows.length) % rows.length : 0);
    } else if (e.key === "Enter" && rows[active]) {
      e.preventDefault();
      void pick(rows[active].full);
    }
  }

  const isCurrent = (full: string) => full.toLowerCase() === current?.toLowerCase();
  const rowId = (i: number) => `${listId}-row-${i}`;
  const caption = !namespaces
    ? null
    : typed
      ? "Checked with Oxen.ai before switching"
      : searching
        ? `${truncated ? `First ${MAX_RESULTS}` : rows.filter((r) => r.kind === "repo").length} matching across all namespaces` +
          (namespace?.can_create ? ` · a new name goes in ${namespace.name}` : "")
        : namespace
          ? namespace.can_create
            ? `${namespace.name} — pick one, or type a new name to create it`
            : `${namespace.name} — shared with you; switching needs editor access`
          : "";

  return (
    <div className={`repo-browser repo-browser-${layout}`} onKeyDown={onKeyDown}>
      <div className="repo-browser-search">
        <Search size={14} aria-hidden="true" />
        <input
          ref={search}
          type="search"
          role="combobox"
          aria-expanded={true}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={active >= 0 ? rowId(active) : undefined}
          aria-busy={busy}
          placeholder={layout === "columns" ? "Search all repositories, or type namespace/name" : "Search, or type namespace/name"}
          aria-label="Search repositories"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          readOnly={busy}
        />
        <IconButton
          aria-label="Refresh the list from Oxen.ai"
          title="Refresh the list from Oxen.ai"
          disabled={loading || busy}
          onClick={() => list(true)}
        >
          <RefreshCw size={14} className={loading ? "repo-browser-spin" : ""} />
        </IconButton>
      </div>

      {(listError || pickError) && (
        <div className="repo-browser-error" role="alert">
          {pickError ?? listError}
        </div>
      )}

      <div className="repo-browser-body">
        {namespaces &&
          (layout === "columns" ? (
            <nav className="repo-browser-namespaces" aria-label="Namespaces">
              {GROUPS.map(({ kind, label }) => {
                const group = namespaces.filter((n) => n.kind === kind);
                if (!group.length) return null;
                return (
                  <div key={kind} className="repo-browser-group">
                    <div className="repo-browser-group-label">{label}</div>
                    {group.map((n) => {
                      const on = n.name === namespace?.name;
                      return (
                        <button
                          key={n.name}
                          type="button"
                          className={`repo-browser-ns ${on ? "on" : ""}`}
                          aria-current={on ? "true" : undefined}
                          title={n.role ? `${n.name} · ${n.role}` : n.name}
                          onClick={() => selectNamespace(n.name)}
                        >
                          <span className="repo-browser-ns-name">{n.name}</span>
                          <span className="repo-browser-count">{n.repos.length}</span>
                        </button>
                      );
                    })}
                  </div>
                );
              })}
            </nav>
          ) : (
            <select
              className="repo-browser-select"
              aria-label="Namespace"
              value={namespace?.name ?? ""}
              onChange={(e) => selectNamespace(e.target.value)}
            >
              {GROUPS.map(({ kind, label }) => {
                const group = namespaces.filter((n) => n.kind === kind);
                return group.length ? (
                  <optgroup key={kind} label={label}>
                    {group.map((n) => (
                      <option key={n.name} value={n.name}>
                        {n.name} ({n.repos.length})
                      </option>
                    ))}
                  </optgroup>
                ) : null;
              })}
            </select>
          ))}

        <div className="repo-browser-results">
          {!namespaces && loading && <div className="repo-browser-empty">Loading repositories from Oxen.ai…</div>}
          {caption && <div className="repo-browser-caption">{caption}</div>}
          <div className="repo-browser-list" id={listId} role="listbox" aria-label="Repositories">
            {rows.map((row, i) => (
              <button
                key={`${row.kind}:${row.full}`}
                id={rowId(i)}
                ref={(el) => {
                  if (el) rowRefs.current.set(i, el);
                  else rowRefs.current.delete(i);
                }}
                type="button"
                role="option"
                tabIndex={-1}
                aria-selected={i === active}
                className={`repo-browser-row ${i === active ? "cursor" : ""} ${isCurrent(row.full) ? "current" : ""}`}
                disabled={busy}
                onMouseEnter={() => setCursor(i)}
                onClick={() => void pick(row.full)}
              >
                {row.kind !== "repo" &&
                  (row.kind === "use" ? (
                    <ArrowRight size={13} className="repo-browser-row-icon" aria-hidden="true" />
                  ) : (
                    <Plus size={13} className="repo-browser-row-icon" aria-hidden="true" />
                  ))}
                <span className="repo-browser-row-text">
                  {row.kind === "repo" ? (
                    <>
                      {searching && <span className="repo-browser-row-ns">{row.namespace}/</span>}
                      {row.repo}
                    </>
                  ) : row.kind === "use" ? (
                    `Use ${row.full}`
                  ) : (
                    `Create ${row.full}`
                  )}
                </span>
                {isCurrent(row.full) ? (
                  <span className="repo-browser-tag">
                    <Check size={12} aria-hidden="true" /> in use
                  </span>
                ) : row.kind === "create" ? (
                  <span className="repo-browser-tag">created on first use</span>
                ) : null}
              </button>
            ))}
          </div>
          {namespaces && !rows.length && (
            <div className="repo-browser-empty">
              {needle ? "Nothing matches. Type namespace/name to use another repository." : "No repositories here yet."}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
