// One generation as a tile: the picture (a video's poster, playing muted on
// hover), a shimmer while it renders (with a cancel), a dashed slot when it
// failed. Shared by the Gallery dock, the project page's media card, and
// the Home page's media lens so a generation looks the same everywhere.
// Every tile is a square filled edge to edge; the full frame is the detail
// view's job, so the grid reads as one even contact sheet.

import { useEffect, useRef, useState, type DragEvent } from "react";
import { Check, Download, Film, TriangleAlert, X } from "lucide-react";
import type { MediaItem } from "../../lib/types";
import { elapsedSince, isInFlight } from "../../lib/media";
import { useAssetSrc } from "../files/useAssetSrc";
import { setDragPaths } from "../files/dnd";
import { downloadLabel, useDownload } from "./useDownload";

export function MediaTile({
  item,
  workspace,
  bust = 0,
  selected = false,
  onSelect,
  onCancel,
  downloadable = false,
}: {
  item: MediaItem;
  /** The project root the item's paths are relative to. */
  workspace: string;
  bust?: number;
  selected?: boolean;
  onSelect?: () => void;
  /** Shown as a × on an in-flight tile; omit to hide the control. */
  onCancel?: () => void;
  /** Show a download button over a finished tile (on hover or focus). */
  downloadable?: boolean;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (selected) ref.current?.scrollIntoView?.({ block: "nearest" });
  }, [selected]);
  const abs = item.path ? `${workspace}/${item.path}` : null;

  if (isInFlight(item)) {
    return (
      <div className="gallery-tile inflight" title={item.prompt} role="status">
        <InFlightLabel item={item} />
        {onCancel && (
          <button
            type="button"
            className="gallery-tile-cancel"
            onClick={onCancel}
            aria-label={`Cancel ${item.kind} generation`}
            title="Cancel"
          >
            <X size={11} />
          </button>
        )}
      </div>
    );
  }
  if (item.status !== "succeeded" || !item.path) {
    return (
      <button
        ref={ref}
        type="button"
        className={`gallery-tile failed ${selected ? "selected" : ""}`}
        onClick={onSelect}
        title={item.error ?? item.status}
      >
        <span className="gallery-tile-status">{item.status.replace("_", " ")}</span>
      </button>
    );
  }
  const tile = (
    <button
      ref={ref}
      type="button"
      className={`gallery-tile ${selected ? "selected" : ""}`}
      onClick={onSelect}
      title={item.prompt}
      draggable
      onDragStart={(e: DragEvent) => abs && setDragPaths(e.dataTransfer, [abs])}
    >
      <TilePreview item={item} workspace={workspace} bust={bust} />
      {item.kind === "video" && (
        <span className="gallery-tile-badge" aria-hidden="true">
          <Film size={11} /> {item.duration_secs ? `${item.duration_secs}s` : "video"}
        </span>
      )}
    </button>
  );
  if (!downloadable) return tile;
  // A button can't hold a button, so the download sits beside the tile in a
  // slot that takes the tile's place in the grid.
  return (
    <div className="gallery-tile-slot">
      {tile}
      <TileDownload workspace={workspace} path={item.path} kind={item.kind} />
    </div>
  );
}

function TileDownload({ workspace, path, kind }: { workspace: string; path: string; kind: MediaItem["kind"] }) {
  const { state, detail, download } = useDownload(workspace, path);
  const Icon = state === "saved" ? Check : state === "failed" ? TriangleAlert : Download;
  return (
    <button
      type="button"
      className={`gallery-tile-download ${state}`}
      onClick={() => void download()}
      disabled={state === "saving"}
      aria-label={state === "idle" ? `Download ${kind}` : downloadLabel(state)}
      title={state === "idle" || state === "saving" ? "Download" : detail}
    >
      <Icon size={12} />
    </button>
  );
}

/** A picture, or a video that plays (muted) on hover with its poster at rest. */
function TilePreview({ item, workspace, bust }: { item: MediaItem; workspace: string; bust: number }) {
  const src = useAssetSrc(workspace, item.path ?? "", bust);
  const poster = useAssetSrc(workspace, item.poster ?? "", bust);
  const videoRef = useRef<HTMLVideoElement>(null);
  if (!src) return <span className="gallery-tile-loading" aria-hidden />;
  if (item.kind === "video") {
    return (
      <video
        ref={videoRef}
        src={src}
        poster={item.poster && poster ? poster : undefined}
        muted
        loop
        playsInline
        preload="metadata"
        onMouseEnter={() => void videoRef.current?.play().catch(() => {})}
        onMouseLeave={() => {
          videoRef.current?.pause();
        }}
      />
    );
  }
  return <img src={src} alt={item.prompt} loading="lazy" draggable={false} />;
}

function InFlightLabel({ item }: { item: MediaItem }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return (
    <span className="gallery-tile-status">
      {item.status} · {elapsedSince(item.created_at, now)}
    </span>
  );
}
