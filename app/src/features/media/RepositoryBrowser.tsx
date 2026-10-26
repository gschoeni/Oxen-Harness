// Choose the Oxen repository a project's assets go to, from what the API key
// can write to: its own namespace, organizations it may create repos in, and
// repositories shared with it. Search spans every namespace at once; with no
// query, one namespace's repositories are listed. A typed `namespace/name`,
// or a new name where the hub will create it, is offered as its own row. The
// backend checks the pick against the hub before saving and its refusal
// (read-only, not a namespace you can create in) is shown in place.
//
// `columns` lays namespaces beside repositories (Settings, where there is
// room); `compact` stacks a namespace select over the list (the Gallery's
// popover).

import { useEffect, useMemo, useRef, useState } from "react";
import { Check, Plus, RefreshCw, Search } from "lucide-react";
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

/** `namespace/name`, normalized, when `text` is one. */
export function asRepo(text: string): string | null {
  const clean = text.trim().replace(/^\/+|\/+$/g, "");
  const [ns, name, ...rest] = clean.split("/");
  return rest.length === 0 && ns && name && NAME.test(ns) && NAME.test(name) ? `${ns}/${name}` : null;
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
  /** The repository in effect (`namespace/name`), highlighted. */
  current: string | null;
  layout: "columns" | "compact";
  /** After a successful switch (or Escape). */
  onDone: () => void;
}) {
  const listed = useStore((s) => s.mediaNamespaces);
  const load = useStore((s) => s.loadMediaNamespaces);
  const choose = useStore((s) => s.chooseMediaDestination);
  const [selected, setSelected] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const search = useRef<HTMLInputElement>(null);

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
    setError(null);
    load(force)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    list(false);
    search.current?.focus();
    // Once per mount; the refresh button forces a re-list.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Start on the namespace of the repository in effect.
  useEffect(() => {
    if (!namespaces || selected) return;
    const home = current?.split("/")[0]?.toLowerCase();
    setSelected((namespaces.find((n) => n.name.toLowerCase() === home) ?? namespaces[0])?.name ?? null);
  }, [namespaces, current, selected]);

  const namespace = namespaces?.find((n) => n.name === selected) ?? null;
  const needle = query.trim().toLowerCase();
  const typed = asRepo(query);

  const rows = useMemo<Row[]>(() => {
    if (!namespaces) return [];
    const out: Row[] = [];
    if (typed) {
      out.push({ kind: "use", full: typed });
      return out;
    }
    const pool = needle ? namespaces : namespace ? [namespace] : [];
    for (const ns of pool) {
      for (const repo of ns.repos) {
        const full = `${ns.name}/${repo}`;
        if (!needle || full.toLowerCase().includes(needle)) out.push({ kind: "repo", full, namespace: ns.name, repo });
        if (out.length >= MAX_RESULTS) break;
      }
    }
    const fresh = query.trim();
    if (
      namespace?.can_create &&
      NAME.test(fresh) &&
      !namespace.repos.some((r) => r.toLowerCase() === fresh.toLowerCase())
    )
      out.push({ kind: "create", full: `${namespace.name}/${fresh}` });
    return out;
  }, [namespaces, namespace, needle, typed, query]);

  useEffect(() => setCursor(0), [needle, selected]);

  async function pick(full: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await choose(root, full);
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  function onKey(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setCursor((c) => (rows.length ? (c + step + rows.length) % rows.length : 0));
    } else if (e.key === "Enter" && rows[cursor]) {
      e.preventDefault();
      void pick(rows[cursor].full);
    } else if (e.key === "Escape") {
      e.stopPropagation();
      onDone();
    }
  }

  const searching = !!needle && !typed;
  const isCurrent = (full: string) => full.toLowerCase() === current?.toLowerCase();

  return (
    <div className={`repo-browser repo-browser-${layout}`}>
      <div className="repo-browser-search">
        <Search size={14} aria-hidden="true" />
        <input
          ref={search}
          type="search"
          placeholder={layout === "columns" ? "Search all repositories, or type namespace/name" : "Search, or type namespace/name"}
          aria-label="Search repositories"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKey}
          disabled={busy}
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

      {error && (
        <div className="repo-browser-error" role="alert">
          {error}
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
                    {group.map((n) => (
                      <button
                        key={n.name}
                        type="button"
                        className={`repo-browser-ns ${n.name === selected && !searching ? "on" : ""}`}
                        aria-pressed={n.name === selected}
                        onClick={() => {
                          setSelected(n.name);
                          setQuery("");
                          search.current?.focus();
                        }}
                      >
                        <span className="repo-browser-ns-name">{n.name}</span>
                        <span className="repo-browser-count">{n.role ?? n.repos.length}</span>
                      </button>
                    ))}
                  </div>
                );
              })}
            </nav>
          ) : (
            <select
              className="repo-browser-select"
              aria-label="Namespace"
              value={selected ?? ""}
              onChange={(e) => {
                setSelected(e.target.value);
                setQuery("");
              }}
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

        <div className="repo-browser-list" role="listbox" aria-label="Repositories">
          {!namespaces && loading && <div className="repo-browser-empty">Loading repositories from Oxen.ai…</div>}
          {namespaces && (
            <div className="repo-browser-caption">
              {typed
                ? "Checked with Oxen.ai before switching"
                : searching
                  ? `${rows.filter((r) => r.kind === "repo").length} matching across all namespaces`
                  : namespace
                    ? namespace.can_create
                      ? `${namespace.name} — pick one, or type a new name to create it`
                      : `${namespace.name} — repositories shared with you`
                    : ""}
            </div>
          )}
          {rows.map((row, i) => (
            <button
              key={`${row.kind}:${row.full}`}
              type="button"
              role="option"
              aria-selected={isCurrent(row.full)}
              className={`repo-browser-row ${i === cursor ? "cursor" : ""} ${isCurrent(row.full) ? "current" : ""}`}
              disabled={busy}
              onMouseEnter={() => setCursor(i)}
              onClick={() => void pick(row.full)}
            >
              {row.kind === "repo" ? (
                <span className="repo-browser-row-name">
                  {searching && <span className="repo-browser-row-ns">{row.namespace}/</span>}
                  {row.repo}
                </span>
              ) : (
                <span className="repo-browser-row-name">
                  <Plus size={13} aria-hidden="true" /> {row.kind === "use" ? `Use ${row.full}` : `Create ${row.full}`}
                </span>
              )}
              {isCurrent(row.full) ? (
                <span className="repo-browser-tag">
                  <Check size={12} aria-hidden="true" /> in use
                </span>
              ) : row.kind === "create" ? (
                <span className="repo-browser-tag">created on first use</span>
              ) : null}
            </button>
          ))}
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
