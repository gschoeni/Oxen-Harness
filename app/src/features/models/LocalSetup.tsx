import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  Check,
  Cpu,
  Download,
  HardDrive,
  Loader,
  Search,
  Sparkles,
  Trash2,
} from "lucide-react";
import {
  detectHardware,
  downloadModel,
  installRuntime,
  installedLocalModels,
  listModelCatalog,
  onModelProgress,
  onRuntimeInstall,
  removeModel,
  runtimeStatus,
} from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { formatBytes } from "../../lib/format";
import type {
  CatalogModel,
  Fit,
  HardwareProfile,
  InstalledView,
  QuantOption,
  RuntimeStatus,
} from "../../lib/types";
import { looksLikeRepo, useHfSearch } from "./useHfSearch";
import "./models.css";

type Tab = "recommended" | "huggingface" | "oxen";

const FIT_LABEL: Record<Fit, string> = {
  good: "Runs well",
  tight: "Tight fit",
  too_big: "Too big",
};

function FitBadge({ fit }: { fit: Fit }) {
  return <span className={`fit-badge fit-${fit}`}>{FIT_LABEL[fit]}</span>;
}

/** Local-model setup, embedded as the "Local models" settings subpage: detect
 *  the machine, set up the runtime, pick a model (curated / Hugging Face / Oxen)
 *  with hardware-fit guidance + auto-quant, download it, and start chatting — all
 *  without the user touching a terminal. Closing the settings surface on "Use"
 *  drops the user straight into the freshly-started local chat. */
export function LocalSetup() {
  const close = () => useStore.getState().setSettingsOpen(false);
  const switchToLocalModel = useStore((s) => s.switchToLocalModel);

  const [hardware, setHardware] = useState<HardwareProfile | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [catalog, setCatalog] = useState<CatalogModel[]>([]);
  const [installed, setInstalled] = useState<InstalledView | null>(null);
  const [tab, setTab] = useState<Tab>("recommended");
  const [selected, setSelected] = useState<CatalogModel | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Runtime install
  const [installingRuntime, setInstallingRuntime] = useState(false);
  const [runtimeLog, setRuntimeLog] = useState<string>("");
  const [runtimePct, setRuntimePct] = useState<number | null>(null);

  // Download (keyed by model id → fraction 0..1)
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [downloadingId, setDownloadingId] = useState<string | null>(null);
  const [usingId, setUsingId] = useState<string | null>(null);

  // Hugging Face — one smart input: type to autocomplete GGUF repos, or paste a
  // repo / .gguf link and load it directly. Its state + search live in the hook.
  const {
    input: hfInput,
    setInput: setHfInput,
    results: hfResults,
    searching: hfSearching,
    open: hfOpen,
    setOpen: setHfOpen,
    active: hfActive,
    setActive: setHfActive,
    resolving,
    hasToken,
    tokenInput,
    setTokenInput,
    showToken,
    setShowToken,
    boxRef: hfBoxRef,
    resolve: doResolve,
    onKeyDown: onHfKeyDown,
    saveToken,
  } = useHfSearch({ onResolved: setSelected, onError: setError });

  const logRef = useRef<HTMLPreElement>(null);

  const refreshAll = () => {
    listModelCatalog().then(setCatalog).catch((e) => setError(String(e)));
    installedLocalModels().then(setInstalled).catch(() => {});
    runtimeStatus().then(setRuntime).catch(() => {});
  };

  useEffect(() => {
    detectHardware().then(setHardware).catch(() => {});
    refreshAll();
    const unProg = onModelProgress((p) =>
      setProgress((prev) => ({ ...prev, [p.id]: p.fraction ?? 0 })),
    );
    const unRt = onRuntimeInstall((e) => {
      if (e.kind === "log") setRuntimeLog((prev) => prev + e.line + "\n");
      else setRuntimePct(e.total ? e.downloaded / e.total : null);
    });
    return () => {
      unProg.then((fn) => fn());
      unRt.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [runtimeLog]);

  const runtimeReady = !!runtime?.binary;

  async function doInstallRuntime() {
    setInstallingRuntime(true);
    setRuntimeLog("");
    setRuntimePct(null);
    setError(null);
    try {
      await installRuntime();
      await runtimeStatus().then(setRuntime);
    } catch (e) {
      setError(`Runtime setup failed: ${e}`);
    } finally {
      setInstallingRuntime(false);
      setRuntimePct(null);
    }
  }

  function pickModel(cm: CatalogModel) {
    setSelected(cm);
    setError(null);
  }

  async function doDownload(q: QuantOption) {
    setDownloadingId(q.model.id);
    setError(null);
    setProgress((p) => ({ ...p, [q.model.id]: 0 }));
    try {
      await downloadModel(q.model);
      refreshAll();
      // Reflect the newly-installed quant in the open panel.
      if (selected) {
        const fresh = await listModelCatalog();
        setCatalog(fresh);
        setSelected(fresh.find((m) => m.id === selected.id) ?? selected);
      }
    } catch (e) {
      setError(`Download failed: ${e}`);
    } finally {
      setDownloadingId(null);
    }
  }

  async function doUse(id: string) {
    setUsingId(id);
    setError(null);
    try {
      await switchToLocalModel(id);
      close();
    } catch (e) {
      setError(`Could not start the model: ${e}`);
      setUsingId(null);
    }
  }

  async function doRemove(id: string) {
    try {
      await removeModel(id);
      refreshAll();
      if (selected) {
        const fresh = await listModelCatalog();
        setCatalog(fresh);
        setSelected(fresh.find((m) => m.id === selected.id) ?? selected);
      }
    } catch (e) {
      setError(`Remove failed: ${e}`);
    }
  }

  const curated = catalog.filter((m) => m.source === "curated");
  const oxen = catalog.filter((m) => m.source === "oxen");

  return (
    <div className="ls-body ls-embedded">
          {error && <div className="ls-error">{error}</div>}

          {/* 1 — Your machine + runtime */}
          <section className="ls-section">
            <div className="ls-step">Your machine</div>
            <div className="ls-machine">
              <MachineStat icon={<Cpu size={15} />} label={hardware?.chip_label ?? "Detecting…"} />
              <MachineStat
                icon={<HardDrive size={15} />}
                label={hardware ? `${formatBytes(hardware.ram_bytes)} memory` : "—"}
              />
              <MachineStat
                icon={<Sparkles size={15} />}
                label={
                  hardware
                    ? hardware.accelerator === "metal"
                      ? "Metal GPU"
                      : hardware.accelerator === "cuda"
                        ? "CUDA GPU"
                        : "CPU only"
                    : "—"
                }
              />
              <div className="ls-runtime">
                {runtimeReady ? (
                  <span className="ls-runtime-ok">
                    <Check size={14} /> Runtime ready
                    {runtime?.source === "managed" && ` (${runtime.managed_version})`}
                  </span>
                ) : runtime?.can_manage ? (
                  <button
                    className="ls-btn ls-btn-primary"
                    onClick={doInstallRuntime}
                    disabled={installingRuntime}
                  >
                    {installingRuntime ? (
                      <>
                        <Loader size={14} className="spin" /> Setting up runtime…
                      </>
                    ) : (
                      <>
                        <Download size={14} /> Set up runtime
                      </>
                    )}
                  </button>
                ) : (
                  <span className="ls-runtime-warn">
                    <AlertTriangle size={14} /> No automatic runtime for this platform
                  </span>
                )}
              </div>
            </div>
            {installingRuntime && (
              <>
                {runtimePct !== null && (
                  <div className="ls-bar">
                    <span style={{ width: `${Math.round(runtimePct * 100)}%` }} />
                  </div>
                )}
                {runtimeLog && (
                  <pre className="ls-log" ref={logRef}>
                    {runtimeLog}
                  </pre>
                )}
              </>
            )}
            {installed?.disk_total != null && installed.disk_free != null && (
              <DiskBar
                total={installed.disk_total}
                free={installed.disk_free}
                models={installed.total_disk_bytes}
              />
            )}
          </section>

          {/* 2 — Choose a model */}
          <section className="ls-section">
            <div className="ls-step">Choose a model</div>
            <div className="ls-tabs">
              {(["recommended", "huggingface", "oxen"] as Tab[]).map((t) => (
                <button
                  key={t}
                  className={`ls-tab ${tab === t ? "active" : ""}`}
                  onClick={() => setTab(t)}
                >
                  {t === "recommended" ? "Recommended" : t === "huggingface" ? "Hugging Face" : "Oxen.ai"}
                </button>
              ))}
            </div>

            {tab === "recommended" && (
              <div className="ls-grid">
                {curated.map((m) => (
                  <ModelCard
                    key={m.id}
                    model={m}
                    active={selected?.id === m.id}
                    onClick={() => pickModel(m)}
                  />
                ))}
              </div>
            )}

            {tab === "huggingface" && (
              <div className="ls-hf">
                <div className="ls-combo" ref={hfBoxRef}>
                  <div className="ls-input-wrap">
                    {resolving ? <Loader size={15} className="spin" /> : <Search size={15} />}
                    <input
                      placeholder="Search Hugging Face, or paste a repo / .gguf link…"
                      value={hfInput}
                      spellCheck={false}
                      autoCapitalize="off"
                      autoCorrect="off"
                      onChange={(e) => setHfInput(e.target.value)}
                      onFocus={() => hfResults.length && setHfOpen(true)}
                      onKeyDown={onHfKeyDown}
                    />
                    {hfSearching && <Loader size={14} className="spin ls-combo-spin" />}
                  </div>

                  {hfOpen && (hfResults.length > 0 || looksLikeRepo(hfInput)) && (
                    <div className="ls-combo-menu" role="listbox">
                      {looksLikeRepo(hfInput) && (
                        <button
                          className={`ls-combo-item ls-combo-load ${hfActive === -1 ? "active" : ""}`}
                          onMouseEnter={() => setHfActive(-1)}
                          onClick={() => doResolve(hfInput)}
                        >
                          <Download size={14} />
                          <span className="ls-hf-name">Load “{hfInput.trim()}”</span>
                        </button>
                      )}
                      {hfResults.map((h, i) => (
                        <button
                          key={h.repo}
                          className={`ls-combo-item ${hfActive === i ? "active" : ""}`}
                          role="option"
                          aria-selected={hfActive === i}
                          onMouseEnter={() => setHfActive(i)}
                          onClick={() => doResolve(h.repo)}
                          title={`Load ${h.repo}`}
                        >
                          <span className="ls-hf-name">{h.repo}</span>
                          <span className="ls-hf-meta">
                            {h.params && <span>{h.params}</span>}
                            <span>↓ {h.downloads.toLocaleString()}</span>
                          </span>
                        </button>
                      ))}
                    </div>
                  )}
                </div>

                <button className="ls-token-toggle" onClick={() => setShowToken((s) => !s)}>
                  {hasToken ? "✓ Hugging Face token saved" : "Add a token for gated models"}
                </button>
                {showToken && (
                  <div className="ls-hf-row">
                    <div className="ls-input-wrap">
                      <input
                        type="password"
                        placeholder="hf_… (stored locally)"
                        value={tokenInput}
                        onChange={(e) => setTokenInput(e.target.value)}
                      />
                    </div>
                    <button className="ls-btn" onClick={saveToken}>
                      Save
                    </button>
                  </div>
                )}
              </div>
            )}

            {tab === "oxen" && (
              <div className="ls-grid">
                {oxen.length > 0 ? (
                  oxen.map((m) => (
                    <ModelCard
                      key={m.id}
                      model={m}
                      active={selected?.id === m.id}
                      onClick={() => pickModel(m)}
                    />
                  ))
                ) : (
                  <p className="ls-empty">
                    Oxen.ai-hosted models are coming soon — pull curated weights straight from
                    Oxen. For now, use the Recommended or Hugging Face tabs.
                  </p>
                )}
              </div>
            )}
          </section>

          {/* 3 — Selected model: quants + download/use */}
          {selected && (
            <section className="ls-section ls-selected">
              <div className="ls-step">
                {selected.display}
                {selected.params && <span className="ls-params"> · {selected.params}</span>}
              </div>
              {selected.note && <p className="ls-note">{selected.note}</p>}
              <div className="ls-quants">
                {selected.quants.map((q) => {
                  const isRec = q.quant === selected.recommended_quant;
                  const dl = downloadingId === q.model.id;
                  const pct = Math.round((progress[q.model.id] ?? 0) * 100);
                  // Warn before a download that won't fit the free disk space.
                  const noSpace =
                    installed?.disk_free != null &&
                    q.size_bytes > 0 &&
                    q.size_bytes > installed.disk_free;
                  return (
                    <div className={`ls-quant ${isRec ? "recommended" : ""}`} key={q.model.id}>
                      <div className="ls-quant-head">
                        <span className="ls-quant-name">{q.quant || "default"}</span>
                        {isRec && <span className="ls-rec">Recommended</span>}
                        <FitBadge fit={q.fit} />
                      </div>
                      <div className="ls-quant-size">{formatBytes(q.size_bytes)}</div>
                      <div className="ls-quant-actions">
                        {q.installed ? (
                          <>
                            <button
                              className="ls-btn ls-btn-primary"
                              onClick={() => doUse(q.model.id)}
                              disabled={!runtimeReady || usingId === q.model.id}
                              title={runtimeReady ? "" : "Set up the runtime first"}
                            >
                              {usingId === q.model.id ? "Starting…" : "Use model"}
                            </button>
                            <button
                              className="ls-icon-btn"
                              onClick={() => doRemove(q.model.id)}
                              aria-label="Remove"
                            >
                              <Trash2 size={15} />
                            </button>
                          </>
                        ) : dl ? (
                          <div className="ls-dl">
                            <div className="ls-bar">
                              <span style={{ width: `${pct}%` }} />
                            </div>
                            <span className="ls-dl-pct">{pct}%</span>
                          </div>
                        ) : noSpace ? (
                          <span className="ls-runtime-warn" title="Free up disk space first">
                            <AlertTriangle size={14} /> Not enough space
                          </span>
                        ) : (
                          <button className="ls-btn" onClick={() => doDownload(q)}>
                            <Download size={14} /> Download
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          {/* 4 — Installed models */}
          {installed && installed.models.length > 0 && (
            <section className="ls-section">
              <div className="ls-step">
                Installed
                <span className="ls-disk">{formatBytes(installed.total_disk_bytes)} on disk</span>
              </div>
              <div className="ls-installed">
                {installed.models.map((m) => (
                  <div className="ls-installed-row" key={m.id}>
                    <Cpu size={14} />
                    <span className="ls-installed-name">{m.display}</span>
                    <span className="ls-installed-size">{formatBytes(m.size_bytes)}</span>
                    <button
                      className="ls-btn ls-btn-primary ls-btn-sm"
                      onClick={() => doUse(m.id)}
                      disabled={!runtimeReady || usingId === m.id}
                    >
                      {usingId === m.id ? "Starting…" : "Use"}
                    </button>
                    <button
                      className="ls-icon-btn"
                      onClick={() => doRemove(m.id)}
                      aria-label={`Remove ${m.display}`}
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                ))}
              </div>
            </section>
          )}
    </div>
  );
}

/** Disk-space bar: total volume size with the model store's usage, other usage,
 *  and free space, so the user can judge a download before starting it. */
function DiskBar({ total, free, models }: { total: number; free: number; models: number }) {
  if (total <= 0) return null;
  const used = Math.max(0, total - free);
  const otherUsed = Math.max(0, used - models);
  const pct = (n: number) => `${Math.min(100, Math.max(0, (n / total) * 100))}%`;
  const lowFree = free < 0.1 * total; // under ~10% free
  return (
    <div className="ls-disk-bar">
      <div className="ls-disk-track" role="img" aria-label="Disk usage">
        <span className="ls-disk-models" style={{ width: pct(models) }} title="Local models" />
        <span className="ls-disk-other" style={{ width: pct(otherUsed) }} title="Other files" />
      </div>
      <div className="ls-disk-legend">
        <span>
          <i className="dot dot-models" /> Models {formatBytes(models)}
        </span>
        <span className={lowFree ? "ls-disk-low" : ""}>
          {formatBytes(free)} free of {formatBytes(total)}
        </span>
      </div>
    </div>
  );
}

function MachineStat({ icon, label }: { icon: React.ReactNode; label: string }) {
  return (
    <span className="ls-machine-stat">
      {icon}
      {label}
    </span>
  );
}

function ModelCard({
  model,
  active,
  onClick,
}: {
  model: CatalogModel;
  active: boolean;
  onClick: () => void;
}) {
  const anyInstalled = model.quants.some((q) => q.installed);
  return (
    <button className={`ls-card ${active ? "active" : ""}`} onClick={onClick}>
      <div className="ls-card-top">
        <span className="ls-card-name">{model.display}</span>
        {anyInstalled && <Check size={14} className="ls-card-installed" />}
      </div>
      {model.params && <span className="ls-card-params">{model.params}</span>}
      <div className="ls-card-bottom">
        <FitBadge fit={model.best_fit} />
        {model.recommended_quant && <span className="ls-card-quant">{model.recommended_quant}</span>}
      </div>
    </button>
  );
}
