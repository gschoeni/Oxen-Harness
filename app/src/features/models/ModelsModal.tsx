import { useEffect, useRef, useState } from "react";
import { Button, Modal } from "../../components/ui";
import {
  installLlama,
  listModels,
  onLlamaInstall,
  onModelProgress,
  pullModel,
  removeModel,
  useLocalModel,
} from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { formatBytes } from "../../lib/format";
import type { ModelsView } from "../../lib/types";
import "./models.css";

export function ModelsModal() {
  const setModelsOpen = useStore((s) => s.setModelsOpen);
  const adoptSession = useStore((s) => s.adoptSession);
  const refreshHistory = useStore((s) => s.refreshHistory);

  const [view, setView] = useState<ModelsView | null>(null);
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installLog, setInstallLog] = useState<string>("");
  const logRef = useRef<HTMLPreElement>(null);

  const refresh = () => listModels().then(setView).catch((e) => setError(String(e)));

  useEffect(() => {
    refresh();
    const unProg = onModelProgress((p) =>
      setProgress((prev) => ({ ...prev, [p.id]: p.fraction ?? 0 })),
    );
    const unLog = onLlamaInstall((line) => setInstallLog((prev) => prev + line + "\n"));
    return () => {
      unProg.then((fn) => fn());
      unLog.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [installLog]);

  async function pull(id: string) {
    setBusyId(id);
    try {
      await pullModel(id);
      await refresh();
    } catch (e) {
      setError(`download failed: ${e}`);
    } finally {
      setBusyId(null);
    }
  }

  async function remove(id: string) {
    try {
      await removeModel(id);
      await refresh();
    } catch (e) {
      setError(`remove failed: ${e}`);
    }
  }

  async function use(id: string) {
    try {
      const info = await useLocalModel(id);
      adoptSession(info); // a local model starts a fresh session
      refreshHistory();
      setModelsOpen(false);
    } catch (e) {
      setError(`could not start: ${e}`);
    }
  }

  async function doInstall() {
    setInstalling(true);
    setInstallLog("");
    try {
      await installLlama();
      setInstallLog((p) => p + "\n✓ Installed. You can now run local models.\n");
      await refresh();
    } catch (e) {
      setInstallLog((p) => p + `\n✕ ${e}\n`);
    } finally {
      setInstalling(false);
    }
  }

  return (
    <Modal title="Local models" wide onClose={() => setModelsOpen(false)}>
      {error && <div className="models-error">{error}</div>}

      {view && !view.llama_installed && (
        <div className="models-warn">
          <div>llama-server isn't installed, so models can be downloaded but not run yet.</div>
          {view.can_auto_install ? (
            <>
              <div className="muted" style={{ marginTop: 4 }}>
                Install it with Homebrew — this can take a few minutes.
              </div>
              <Button size="sm" onClick={doInstall} disabled={installing} style={{ marginTop: 12 }}>
                {installing ? "Installing…" : "Install llama.cpp"}
              </Button>
              {installLog && (
                <pre className="install-log" ref={logRef}>
                  {installLog}
                </pre>
              )}
            </>
          ) : (
            <div style={{ marginTop: 4 }}>{view.install_hint}</div>
          )}
        </div>
      )}

      <div className="models-list">
        {view?.models.map((m) => (
          <div className="model-row" key={m.id}>
            <div className="model-name">{m.display}</div>
            <div className="model-actions">
              <span className={`pill ${m.installed ? "installed" : ""}`}>
                {m.installed ? "● on disk" : "○ not yet"}
              </span>
              {m.installed ? (
                <>
                  <Button size="sm" onClick={() => use(m.id)} disabled={!view.llama_installed}>
                    Use
                  </Button>
                  <Button size="sm" variant="danger" onClick={() => remove(m.id)}>
                    Remove
                  </Button>
                </>
              ) : (
                <Button size="sm" onClick={() => pull(m.id)} disabled={busyId === m.id}>
                  {busyId === m.id ? "Downloading…" : "Download"}
                </Button>
              )}
            </div>
            <div className="model-sub">
              {m.params} · {m.quant} · {formatBytes(m.size_bytes)} · {m.note}
            </div>
            {busyId === m.id && (
              <div className="bar">
                <span style={{ width: `${Math.round((progress[m.id] ?? 0) * 100)}%` }} />
              </div>
            )}
          </div>
        ))}
      </div>

      {view && (
        <div className="models-foot">
          <span className="muted">Disk used: {formatBytes(view.total_disk_bytes)}</span>
          <span className="muted dir">{view.dir}</span>
        </div>
      )}
    </Modal>
  );
}
