// Settings → Media → On-device models: the image models that run on this
// Mac (free, offline, no key), with download progress, removal, and the
// engine and disk they depend on. Installed ones also appear in the default
// image model picker below, so "use it by default" is one more click.

import { useCallback, useEffect, useState } from "react";
import { Check, Download, HardDrive, Trash2, X } from "lucide-react";
import { Button } from "../../components/ui";
import {
  cancelLocalMediaPull,
  localMediaStatus,
  onLocalMediaProgress,
  pullLocalMedia,
  removeLocalMedia,
} from "../../lib/ipc";
import { gb } from "../../lib/memory";
import type { LocalMediaModel, LocalMediaProgressEvent, LocalMediaStatus } from "../../lib/types";

export function LocalMediaSection({ onChanged }: { onChanged?: () => void }) {
  const [status, setStatus] = useState<LocalMediaStatus | null>(null);
  const [progress, setProgress] = useState<Record<string, LocalMediaProgressEvent>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  const refresh = useCallback(() => {
    localMediaStatus()
      .then(setStatus)
      .catch((e) => setStatus((s) => s ?? emptyStatus(String(e))));
  }, []);

  useEffect(() => {
    refresh();
    let off: (() => void) | undefined;
    let dead = false;
    void onLocalMediaProgress((e) => setProgress((p) => ({ ...p, [e.id]: e }))).then((un) => {
      if (dead) un();
      else off = un;
    });
    return () => {
      dead = true;
      off?.();
    };
  }, [refresh]);

  const pull = (m: LocalMediaModel) => {
    setErrors(({ [m.id]: _, ...rest }) => rest);
    setProgress((p) => ({
      ...p,
      [m.id]: { id: m.id, phase: "weights", file: null, done_bytes: m.total_bytes - m.missing_bytes, total_bytes: m.total_bytes, error: null },
    }));
    pullLocalMedia(m.id)
      .then(() => onChanged?.())
      .catch((e) => {
        const message = String(e);
        if (!/cancel/i.test(message)) setErrors((x) => ({ ...x, [m.id]: message }));
      })
      .finally(() => {
        setProgress(({ [m.id]: _, ...rest }) => rest);
        refresh();
      });
  };

  const remove = (m: LocalMediaModel) => {
    removeLocalMedia(m.id)
      .then(() => onChanged?.())
      .catch((e) => setErrors((x) => ({ ...x, [m.id]: String(e) })))
      .finally(refresh);
  };

  return (
    <section className="settings-section local-media">
      <div className="settings-label">On-device models</div>
      <p className="hint">
        Image models that run on this Mac: free, private, and they work offline. The agent uses one
        when you ask for it by name or pick it as the default below. A model that would push your
        memory too far is stopped automatically.
      </p>
      {status?.error && <p className="save-status err">Local models are unavailable: {status.error}</p>}
      <div className="local-media-list">
        {status?.models.map((m) => (
          <LocalModelRow
            key={m.id}
            model={m}
            progress={progress[m.id]}
            error={errors[m.id]}
            onPull={() => pull(m)}
            onCancel={() => void cancelLocalMediaPull(m.id)}
            onRemove={() => remove(m)}
          />
        ))}
      </div>
      {status && !status.error && (
        <div className="local-media-foot">
          <HardDrive size={13} aria-hidden />
          <span>
            {status.disk_available_bytes != null ? `${gb(status.disk_available_bytes)} GB free` : "Free space unknown"}
            {" · "}
            {status.engine_version ? `engine stable-diffusion.cpp ${status.engine_version}` : "the engine downloads with the first model"}
          </span>
        </div>
      )}
    </section>
  );
}

function emptyStatus(error: string): LocalMediaStatus {
  return { models: [], engine_version: null, store_path: "", disk_available_bytes: null, error };
}

function LocalModelRow({
  model,
  progress,
  error,
  onPull,
  onCancel,
  onRemove,
}: {
  model: LocalMediaModel;
  progress?: LocalMediaProgressEvent;
  error?: string;
  onPull: () => void;
  onCancel: () => void;
  onRemove: () => void;
}) {
  const pulling = !!progress && (progress.phase === "weights" || progress.phase === "runtime");
  const pct = pulling && progress.total_bytes > 0 ? Math.round((progress.done_bytes / progress.total_bytes) * 100) : 0;
  const facts = [
    model.developer,
    model.license,
    `${gb(model.total_bytes)} GB`,
    model.typical_secs ? `~${Math.round(model.typical_secs)}s per image` : null,
    model.takes_references ? "edits references" : null,
  ].filter(Boolean);
  return (
    <div className="local-media-row" data-installed={model.installed || undefined}>
      <div className="local-media-info">
        <div className="local-media-name">
          {model.display_name}
          <code className="local-media-id">{model.id}</code>
        </div>
        <div className="local-media-facts">{facts.join(" · ")}</div>
        {model.summary && <div className="local-media-summary">{model.summary}</div>}
        {pulling && (
          <div className="local-media-progress">
            <div
              className="media-upload-track"
              role="progressbar"
              aria-label={`Downloading ${model.display_name}`}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct}
            >
              <div className="media-upload-fill" style={{ width: `${pct}%` }} />
            </div>
            <span className="local-media-progress-text">
              {progress.phase === "runtime" ? "Fetching the engine" : "Downloading"} · {pct}% ·{" "}
              {gb(progress.done_bytes)} of {gb(progress.total_bytes)} GB
            </span>
          </div>
        )}
        {error && <div className="save-status err">{error}</div>}
      </div>
      <div className="local-media-actions">
        {pulling ? (
          <Button variant="outline" size="sm" onClick={onCancel} aria-label={`Cancel downloading ${model.display_name}`}>
            <X size={13} /> Cancel
          </Button>
        ) : model.installed ? (
          <>
            <span className="local-media-installed">
              <Check size={13} /> Installed
            </span>
            <Button variant="outline" size="sm" onClick={onRemove} aria-label={`Remove ${model.display_name}`}>
              <Trash2 size={13} />
            </Button>
          </>
        ) : (
          <Button variant="primary" size="sm" onClick={onPull} aria-label={`Download ${model.display_name}`}>
            <Download size={13} /> {model.missing_bytes < model.total_bytes ? "Finish" : "Download"} ·{" "}
            {gb(model.missing_bytes)} GB
          </Button>
        )}
      </div>
    </div>
  );
}
