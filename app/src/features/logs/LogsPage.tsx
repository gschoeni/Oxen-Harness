// Training-data builder. Browse every chat (trace), mark each Keep or Reject for
// the fine-tuning dataset (persisted per chat), and export the kept ones as
// Oxen.ai chat-completions JSONL:
// https://docs.oxen.ai/examples/fine-tuning/chat_completions
//
// Filters (search / model / project / min length / status) narrow the list, and
// bulk actions apply Keep/Reject/Clear to the whole filtered set at once — so you
// can e.g. "keep every chat from model X with 6+ messages" in two clicks. Quick
// per-row toggles and a click-to-review drawer handle the fine-grained pass.

import { useCallback, useEffect, useMemo, useState } from "react";
import { Ban, Check, Download, FileText, Import, RotateCcw, Search } from "lucide-react";
import { Button } from "../../components/ui";
import { exportFinetuning, importExternal, importSourcesScan, pickExportPath } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { ImportSourceStatus, ReviewStatus, SessionSummary } from "../../lib/types";
import "./logs.css";

/** Display names for the importable sources (`SessionSummary.source` values). */
const SOURCE_LABELS: Record<string, string> = {
  "claude-code": "Claude Code",
  cursor: "Cursor",
};

type Filter = "all" | "unreviewed" | "kept" | "rejected";

const FILTERS: { key: Filter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "unreviewed", label: "Unreviewed" },
  { key: "kept", label: "Kept" },
  { key: "rejected", label: "Rejected" },
];

const LENGTHS: { value: number; label: string }[] = [
  { value: 0, label: "Any length" },
  { value: 3, label: "3+ messages" },
  { value: 6, label: "6+ messages" },
  { value: 10, label: "10+ messages" },
  { value: 20, label: "20+ messages" },
];

function matchesStatus(filter: Filter, status: ReviewStatus): boolean {
  if (filter === "all") return true;
  if (filter === "unreviewed") return status === "";
  return status === filter;
}

/** Last path segment of a workspace, for a compact project label. */
function baseName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] || path;
}

export function LogsPage() {
  const sessions = useStore((s) => s.sessions);
  const refreshHistory = useStore((s) => s.refreshHistory);
  const setReviewStatus = useStore((s) => s.setReviewStatus);
  const setReviewStatusMany = useStore((s) => s.setReviewStatusMany);
  const openReview = useStore((s) => s.openReview);

  const [status, setStatus] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  const [model, setModel] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [source, setSource] = useState("");
  const [minMessages, setMinMessages] = useState(0);
  const [includeTools, setIncludeTools] = useState(true);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ ok: boolean; msg: string } | null>(null);

  // What Claude Code / Cursor have on this machine, for the import panel.
  const [importSources, setImportSources] = useState<ImportSourceStatus[]>([]);
  const [importing, setImporting] = useState<string | null>(null);

  const rescanSources = useCallback(() => {
    importSourcesScan()
      .then(setImportSources)
      .catch(() => setImportSources([]));
  }, []);

  useEffect(() => {
    refreshHistory();
    rescanSources();
  }, [refreshHistory, rescanSources]);

  async function doImport(src: string) {
    setNotice(null);
    setImporting(src);
    try {
      const report = await importExternal(src);
      const label = SOURCE_LABELS[src] ?? src;
      setNotice({
        ok: true,
        msg:
          report.imported === 0 && report.updated === 0
            ? `${label}: nothing new — ${report.skipped} conversation${report.skipped === 1 ? "" : "s"} already imported.`
            : `${label}: imported ${report.imported} new, refreshed ${report.updated}, ${report.skipped} unchanged.`,
      });
      await refreshHistory();
      rescanSources();
    } catch (e) {
      setNotice({ ok: false, msg: String(e) });
    } finally {
      setImporting(null);
    }
  }

  // Distinct models / workspaces present, for the dropdowns.
  const models = useMemo(
    () => [...new Set(sessions.map((s) => s.model))].sort(),
    [sessions],
  );
  const workspaces = useMemo(
    () => [...new Set(sessions.map((s) => s.workspace))].sort(),
    [sessions],
  );

  const counts = useMemo(() => {
    let kept = 0;
    let rejected = 0;
    let unreviewed = 0;
    for (const s of sessions) {
      if (s.review_status === "kept") kept++;
      else if (s.review_status === "rejected") rejected++;
      else unreviewed++;
    }
    return { kept, rejected, unreviewed };
  }, [sessions]);

  const shown = useMemo(() => {
    const q = search.trim().toLowerCase();
    return sessions.filter(
      (s) =>
        matchesStatus(status, s.review_status) &&
        (model === "" || s.model === model) &&
        (workspace === "" || s.workspace === workspace) &&
        (source === "" || (source === "native" ? s.source === "" : s.source === source)) &&
        s.message_count >= minMessages &&
        (q === "" || (s.title ?? "").toLowerCase().includes(q)),
    );
  }, [sessions, status, model, workspace, source, minMessages, search]);

  const activeFilters =
    model !== "" || workspace !== "" || source !== "" || minMessages > 0 || search.trim() !== "";
  const hasImported = useMemo(() => sessions.some((s) => s.source !== ""), [sessions]);

  function resetFilters() {
    setSearch("");
    setModel("");
    setWorkspace("");
    setSource("");
    setMinMessages(0);
    setStatus("all");
  }

  async function bulk(next: ReviewStatus) {
    const ids = shown.map((s) => s.id);
    if (ids.length === 0) return;
    await setReviewStatusMany(ids, next);
  }

  async function doExport() {
    const keptIds = sessions.filter((s) => s.review_status === "kept").map((s) => s.id);
    if (keptIds.length === 0) return;
    setNotice(null);
    const path = await pickExportPath(`oxen-finetuning-${keptIds.length}-chats.jsonl`);
    if (!path) return;
    setBusy(true);
    try {
      const count = await exportFinetuning(path, keptIds, includeTools);
      setNotice(
        count === 0
          ? { ok: false, msg: "No usable conversations in the kept chats (each needs a user + assistant turn)." }
          : { ok: true, msg: `Exported ${count} conversation${count === 1 ? "" : "s"} → ${path}` },
      );
    } catch (e) {
      setNotice({ ok: false, msg: String(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="settings-page">
      <section className="settings-section log-intro">
        <div className="log-header-text">
          <div className="settings-label">Training dataset</div>
          <p className="hint">
            Curate chats into a fine-tuning set for{" "}
            <a href="https://docs.oxen.ai/examples/fine-tuning/chat_completions" target="_blank" rel="noreferrer">
              Oxen.ai
            </a>
            . Filter, then Keep or Reject — export ships every chat marked Keep.
          </p>
        </div>

        <div className="log-summary">
          <span className="log-summary-item">
            <span className="log-dot kept" />
            <b>{counts.kept}</b> kept
          </span>
          <span className="log-summary-item">
            <span className="log-dot rejected" />
            <b>{counts.rejected}</b> rejected
          </span>
          <span className="log-summary-item">
            <span className="log-dot" />
            <b>{counts.unreviewed}</b> unreviewed
          </span>
        </div>

        <div className="log-export">
          <Button variant="primary" size="sm" onClick={doExport} disabled={busy || counts.kept === 0}>
            <Download size={15} />
            {busy ? "Exporting…" : `Export ${counts.kept} kept chat${counts.kept === 1 ? "" : "s"}`}
          </Button>
          <label className="log-toggle" title="Preserve tool calls and results in the export">
            <input
              type="checkbox"
              checked={includeTools}
              onChange={(e) => setIncludeTools(e.target.checked)}
            />
            Include tools
          </label>
        </div>
        {notice && <span className={`save-status ${notice.ok ? "ok" : "err"}`}>{notice.msg}</span>}
      </section>

      {importSources.some((s) => s.available > 0 || s.imported > 0) && (
        <section className="settings-section log-import">
          <div className="settings-label">Import from other tools</div>
          <p className="hint">
            Pull conversations from coding tools on this machine into the dataset builder. Rescans
            only add what&apos;s new; imported chats are review-only and keep their tool calls and
            thinking.
          </p>
          {importSources
            .filter((s) => s.available > 0 || s.imported > 0)
            .map((s) => (
              <div key={s.source} className="log-import-row">
                <span className="log-import-name">{SOURCE_LABELS[s.source] ?? s.source}</span>
                <span className="log-import-meta">
                  {s.available} conversation{s.available === 1 ? "" : "s"} found
                  {s.imported > 0 && <> · {s.imported} imported</>}
                </span>
                <Button
                  size="sm"
                  onClick={() => doImport(s.source)}
                  disabled={importing !== null || s.available === 0}
                >
                  <Import size={15} />
                  {importing === s.source ? "Importing…" : s.imported > 0 ? "Rescan" : "Import all"}
                </Button>
              </div>
            ))}
        </section>
      )}

      <section className="settings-section">
        {/* Filters */}
        <div className="log-filterbar">
          <div className="log-search">
            <Search size={15} />
            <input
              placeholder="Search titles…"
              value={search}
              spellCheck={false}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <select className="log-select" value={model} onChange={(e) => setModel(e.target.value)}>
            <option value="">All models</option>
            {models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
          {hasImported && (
            <select className="log-select" value={source} onChange={(e) => setSource(e.target.value)}>
              <option value="">All sources</option>
              <option value="native">This app</option>
              {Object.entries(SOURCE_LABELS).map(([key, label]) => (
                <option key={key} value={key}>
                  {label}
                </option>
              ))}
            </select>
          )}
          {workspaces.length > 1 && (
            <select
              className="log-select"
              value={workspace}
              onChange={(e) => setWorkspace(e.target.value)}
              title={workspace || "All projects"}
            >
              <option value="">All projects</option>
              {workspaces.map((w) => (
                <option key={w} value={w}>
                  {baseName(w)}
                </option>
              ))}
            </select>
          )}
          <select
            className="log-select"
            value={minMessages}
            onChange={(e) => setMinMessages(Number(e.target.value))}
          >
            {LENGTHS.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </select>
          {activeFilters && (
            <button className="log-clear-filters" onClick={resetFilters}>
              <RotateCcw size={13} /> Clear filters
            </button>
          )}
        </div>

        <div className="log-toolbar">
          <div className="segmented" role="tablist">
            {FILTERS.map((f) => (
              <button
                key={f.key}
                role="tab"
                aria-selected={status === f.key}
                className={status === f.key ? "active" : ""}
                onClick={() => setStatus(f.key)}
              >
                {f.label}
              </button>
            ))}
          </div>

          {/* Bulk actions apply to the whole filtered list. */}
          <div className="log-bulk">
            <span className="log-bulk-count">{shown.length} shown</span>
            <button className="log-bulk-btn keep" onClick={() => bulk("kept")} disabled={shown.length === 0}>
              Keep all
            </button>
            <button className="log-bulk-btn reject" onClick={() => bulk("rejected")} disabled={shown.length === 0}>
              Reject all
            </button>
            <button className="log-bulk-btn" onClick={() => bulk("")} disabled={shown.length === 0}>
              Clear
            </button>
          </div>
        </div>

        <div className="log-trace-list">
          {sessions.length === 0 ? (
            <p className="muted">No chats yet — start a conversation to build a dataset.</p>
          ) : shown.length === 0 ? (
            <p className="muted">No chats match these filters.</p>
          ) : (
            shown.map((s, i) => (
              <TraceRow
                key={s.id}
                trace={s}
                onKeep={() => setReviewStatus(s.id, s.review_status === "kept" ? "" : "kept")}
                onReject={() => setReviewStatus(s.id, s.review_status === "rejected" ? "" : "rejected")}
                onOpen={() => openReview(shown.map((x) => x.id), i)}
              />
            ))
          )}
        </div>
      </section>
    </div>
  );
}

function TraceRow({
  trace,
  onKeep,
  onReject,
  onOpen,
}: {
  trace: SessionSummary;
  onKeep: () => void;
  onReject: () => void;
  onOpen: () => void;
}) {
  const status = trace.review_status;
  return (
    <div className={`log-trace status-${status || "none"}`}>
      <button className="log-trace-main" onClick={onOpen} title="Open transcript to review">
        <FileText size={15} className="log-trace-icon" />
        <span className="log-trace-text">
          <span className="log-trace-title">{trace.title ?? "(untitled chat)"}</span>
          <span className="log-trace-meta">
            {trace.source && (
              <span className="log-source-badge">{SOURCE_LABELS[trace.source] ?? trace.source}</span>
            )}
            {trace.model} · {trace.message_count} msg
          </span>
        </span>
        {status && <span className={`log-pill log-pill-${status}`}>{status}</span>}
      </button>
      <div className="log-trace-actions">
        <button
          className={`log-decide keep ${status === "kept" ? "active" : ""}`}
          onClick={onKeep}
          title={status === "kept" ? "Kept — click to unmark" : "Keep for the dataset"}
          aria-label="Keep"
        >
          <Check size={15} />
        </button>
        <button
          className={`log-decide reject ${status === "rejected" ? "active" : ""}`}
          onClick={onReject}
          title={status === "rejected" ? "Rejected — click to unmark" : "Reject from the dataset"}
          aria-label="Reject"
        >
          <Ban size={15} />
        </button>
      </div>
    </div>
  );
}
