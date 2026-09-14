// Image/video generations of this chat that are still rendering, and the
// reference files on their way to the hub before one starts: one row per
// upload (filename, a slim progress bar, percent · sent of total) above one
// row per job (what it is, how long it has been going) with a cancel.
// Appears only while something is in flight, fed by the project's
// `media://changed` feed (the whole library plus live uploads, filtered to
// this chat).

import { useEffect, useState } from "react";
import { Film, Image as ImageIcon, Music, Sparkles, X } from "lucide-react";
import { useStore } from "../../lib/store";
import { elapsedSince, fmtBytes, isActiveUpload, isInFlight, uploadPercent } from "../../lib/media";
import type { MediaUpload } from "../../lib/types";

const NONE: never[] = [];

export function MediaPanel() {
  const sessionId = useStore((s) => s.session?.session_id);
  const workspace = useStore((s) => s.session?.workspace);
  const items = useStore((s) => (s.session?.workspace ? s.media[s.session.workspace] : undefined)) ?? NONE;
  const uploads = useStore((s) => (s.session?.workspace ? s.mediaUploads[s.session.workspace] : undefined)) ?? NONE;
  const refreshMedia = useStore((s) => s.refreshMedia);
  const cancelMedia = useStore((s) => s.cancelMedia);
  const openGallery = useStore((s) => s.openGallery);
  useEffect(() => {
    if (workspace) void refreshMedia(workspace);
  }, [workspace, refreshMedia]);
  const inFlight = items.filter((i) => i.session === sessionId && isInFlight(i));
  const active = uploads.filter((u) => u.session === sessionId && isActiveUpload(u));
  const uploading = active.filter((u) => u.status !== "failed").length;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!inFlight.length) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [inFlight.length]);
  if (!sessionId || (inFlight.length === 0 && active.length === 0)) return null;
  const title =
    uploading > 0
      ? `Uploading ${uploading === 1 ? "1 reference" : `${uploading} references`}…`
      : inFlight.length === 1
        ? "1 generation rendering"
        : inFlight.length > 1
          ? `${inFlight.length} generations rendering`
          : "A reference upload failed";
  return (
    <div className="fleet-panel tasks-panel media-panel" role="status" aria-label="Generations in progress">
      <div className="fleet-panel-head">
        <Sparkles size={13} className="fleet-panel-icon" />
        <span className="fleet-panel-title">{title}</span>
        <button type="button" className="fleet-panel-hint media-panel-link" onClick={() => openGallery()}>
          open the gallery
        </button>
      </div>
      <div className="fleet-lanes">
        {active.map((u) => (
          <UploadRow key={u.id} upload={u} />
        ))}
        {inFlight.map((item) => (
          <div key={item.id} className="fleet-lane-row">
            <div className="fleet-lane tasks-row" title={item.prompt}>
              <span className="fleet-glyph running" aria-label={item.status} />
              <span className="fleet-lane-name tasks-command">
                {item.kind} · {item.model}
              </span>
              <span className="fleet-lane-activity">{item.prompt}</span>
              <span className="fleet-lane-tokens">{elapsedSince(item.created_at, now)}</span>
            </div>
            <button
              type="button"
              className="fleet-lane-stop"
              onClick={() => cancelMedia(item.id)}
              title="Cancel this generation"
              aria-label={`Cancel ${item.kind} ${item.index}`}
            >
              <X size={11} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

/** One reference file on its way to the hub: icon, name, bar, percent. */
export function UploadRow({ upload, compact = false }: { upload: MediaUpload; compact?: boolean }) {
  const Icon = upload.kind === "video" ? Film : upload.kind === "audio" ? Music : ImageIcon;
  const percent = uploadPercent(upload);
  const failed = upload.status === "failed";
  const presigning = upload.status === "presigning";
  const status = failed
    ? (upload.error ?? "upload failed")
    : presigning
      ? "finishing…"
      : `${percent}% · ${fmtBytes(upload.bytes_sent)} of ${fmtBytes(upload.bytes_total)}`;
  return (
    <div
      className={`media-upload ${failed ? "failed" : ""} ${compact ? "compact" : ""}`}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={failed ? undefined : percent}
      aria-label={`Uploading ${upload.filename}`}
      title={upload.error ?? upload.filename}
    >
      <div className="media-upload-line">
        <Icon size={13} className="media-upload-icon" aria-hidden="true" />
        <span className="media-upload-name">{upload.filename}</span>
        {upload.label && <span className="media-upload-label">{upload.label}</span>}
        <span className="media-upload-status">{status}</span>
      </div>
      <div className={`media-upload-track ${presigning ? "indeterminate" : ""}`}>
        <div className="media-upload-fill" style={{ width: `${failed ? 100 : percent}%` }} />
      </div>
    </div>
  );
}
