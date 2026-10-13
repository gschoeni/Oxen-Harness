// A rendering generation's progress: a bar that fills step by step while a
// local model samples, sweeps while it loads or decodes, and a label saying
// which. Hosted generations report no steps, so they get the sweep and
// their queue status. A memory warning rides beneath the bar.

import { AlertTriangle } from "lucide-react";
import type { MediaItem } from "../../lib/types";

export function progressLabel(item: MediaItem): string {
  const p = item.progress;
  if (!p) return item.status === "queued" ? "Queued" : "Rendering";
  switch (p.phase) {
    case "queued":
      return "Waiting for the GPU";
    case "loading":
      return "Loading the model";
    case "sampling":
      return p.step != null && p.total ? `Step ${p.step} of ${p.total}` : "Sampling";
    case "finishing":
      return "Decoding the image";
    default:
      return p.phase;
  }
}

/** 0–100 when the generation reports steps; `null` for an indeterminate bar. */
export function progressPercent(item: MediaItem): number | null {
  const p = item.progress;
  if (!p || p.phase !== "sampling" || p.step == null || !p.total) return null;
  return Math.round(Math.min(1, p.step / p.total) * 100);
}

export function GenerationProgress({ item, compact = false }: { item: MediaItem; compact?: boolean }) {
  const percent = progressPercent(item);
  const label = progressLabel(item);
  return (
    <div className={`generation-progress ${compact ? "compact" : ""}`}>
      <div
        className={`media-upload-track ${percent === null ? "indeterminate" : ""}`}
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent ?? undefined}
      >
        <div className="media-upload-fill" style={{ width: `${percent ?? 0}%` }} />
      </div>
      <div className="generation-progress-line">
        <span className="generation-progress-label">{label}</span>
        {percent !== null && <span className="generation-progress-pct">{percent}%</span>}
      </div>
      {item.progress?.note && (
        <div className="generation-progress-note">
          <AlertTriangle size={12} aria-hidden />
          <span>{item.progress.note}</span>
        </div>
      )}
    </div>
  );
}
