// The Gallery dock: the project's generated images and videos as a feed,
// newest first, with what's still rendering at the top. Fed by the project's
// `media://changed` events (the whole library each time) plus a cold load,
// and re-read when the watcher sees the output folder change (a file edited
// or deleted outside the app). Selecting a tile opens a detail pane below the
// grid: the prompt, model, parameters, cost, references, and the actions that
// feed the next prompt — "Use as reference" stages the file for the composer.

import { useEffect, useMemo, useState, type DragEvent, type PointerEvent } from "react";
import { Check, Copy, FileCode2, FolderTree, Images, Paperclip, X } from "lucide-react";
import { useStore } from "../../lib/store";
import type { MediaItem } from "../../lib/types";
import { fmtUsd, isInFlight, whenLabel } from "../../lib/media";
import { useAssetSrc } from "../files/useAssetSrc";
import { useFsChangedUnder } from "../files/useFsChanged";
import { MediaTile } from "./MediaTile";
import { setDragPaths } from "../files/dnd";
import "./media.css";

type Filter = "all" | "image" | "video";
const NONE: MediaItem[] = [];

export function GalleryPanel({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const sessionId = useStore((s) => s.session?.session_id);
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const items = useStore((s) => (s.session?.workspace ? s.media[s.session.workspace] : undefined)) ?? NONE;
  const refreshMedia = useStore((s) => s.refreshMedia);
  const mediaFocus = useStore((s) => s.mediaFocus);
  const clearMediaFocus = useStore((s) => s.clearMediaFocus);
  const cancelMedia = useStore((s) => s.cancelMedia);
  const [filter, setFilter] = useState<Filter>("all");
  const [thisChat, setThisChat] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [bust, setBust] = useState(0);

  useEffect(() => {
    if (workspace) void refreshMedia(workspace);
  }, [workspace, refreshMedia]);
  // Files changed under the output folder (deleted, replaced): re-read the
  // manifest and refresh the pixels.
  const outputDir = useMemo(() => {
    const first = items.find((i) => i.path)?.path ?? "generations/";
    const i = first.indexOf("/");
    return i > 0 ? first.slice(0, i + 1) : "generations/";
  }, [items]);
  useFsChangedUnder(workspace ?? "", outputDir, () => {
    setBust((b) => b + 1);
    if (workspace) void refreshMedia(workspace);
  });

  // A chat card asked for one item: select it and scroll it into view.
  useEffect(() => {
    if (!mediaFocus) return;
    setSelected(mediaFocus);
    setFilter("all");
    setThisChat(false);
    clearMediaFocus();
  }, [mediaFocus, clearMediaFocus]);

  const shown = useMemo(
    () =>
      items.filter(
        (i) =>
          (filter === "all" || i.kind === filter) &&
          (!thisChat || i.session === sessionId) &&
          i.status !== "cancelled",
      ),
    [items, filter, thisChat, sessionId],
  );
  const detail = selected ? items.find((i) => i.id === selected) ?? null : null;
  const inFlight = items.filter(isInFlight).length;
  const uploading = useStore((s) => (workspace ? s.mediaUploads[workspace] : undefined))?.filter(
    (u) => u.status === "uploading" || u.status === "presigning",
  ).length ?? 0;

  if (!workspace) return null;
  return (
    <aside className="canvas gallery" aria-label="Gallery">
      {onResizeStart && <div className="canvas-resizer" onPointerDown={onResizeStart} />}
      <header className="canvas-head gallery-head">
        <div className="gallery-title">
          <Images size={14} aria-hidden="true" />
          <span>
            {shown.length} {shown.length === 1 ? "generation" : "generations"}
            {inFlight > 0 && <span className="gallery-inflight"> · {inFlight} rendering</span>}
            {uploading > 0 && <span className="gallery-inflight"> · uploading {uploading}</span>}
          </span>
        </div>
        <div className="gallery-filters" role="group" aria-label="Filter">
          {(["all", "image", "video"] as Filter[]).map((f) => (
            <button
              key={f}
              type="button"
              className={`gallery-chip ${filter === f ? "on" : ""}`}
              onClick={() => setFilter(f)}
              aria-pressed={filter === f}
            >
              {f === "all" ? "All" : f === "image" ? "Images" : "Videos"}
            </button>
          ))}
          <button
            type="button"
            className={`gallery-chip ${thisChat ? "on" : ""}`}
            onClick={() => setThisChat((v) => !v)}
            aria-pressed={thisChat}
            title="Only generations from this chat"
          >
            This chat
          </button>
        </div>
      </header>
      {shown.length === 0 ? (
        <p className="gallery-empty">
          Nothing here yet. Ask the agent for an image or a video and it lands in{" "}
          <code>{outputDir}</code>.
        </p>
      ) : (
        <div className="gallery-scroll">
          <div className="gallery-grid">
            {shown.map((item) => (
              <MediaTile
                key={item.id}
                item={item}
                workspace={workspace}
                bust={bust}
                selected={item.id === selected}
                onSelect={() => setSelected(item.id === selected ? null : item.id)}
                onCancel={() => cancelMedia(item.id)}
              />
            ))}
          </div>
        </div>
      )}
      {detail && <DetailPane item={detail} workspace={workspace} bust={bust} onClose={() => setSelected(null)} />}
    </aside>
  );
}

function DetailPane({
  item,
  workspace,
  bust,
  onClose,
}: {
  item: MediaItem;
  workspace: string;
  bust: number;
  onClose: () => void;
}) {
  const src = useAssetSrc(workspace, item.path ?? "", bust);
  const poster = useAssetSrc(workspace, item.poster ?? "", bust);
  const stageAttachment = useStore((s) => s.stageAttachment);
  const openInViewer = useStore((s) => s.openInViewer);
  const revealInFiles = useStore((s) => s.revealInFiles);
  const [copied, setCopied] = useState(false);
  const params = Object.entries(item.params ?? {}).filter(([, v]) => v !== null && v !== undefined && v !== "");
  const parent = useStore((s) =>
    item.parent && s.session?.workspace ? s.media[s.session.workspace]?.find((m) => m.path === item.parent) : undefined,
  );
  const copyPrompt = () => {
    void navigator.clipboard?.writeText(item.prompt).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <div className="gallery-detail" aria-label="Generation details">
      <div className="gallery-detail-preview">
        {item.path && src ? (
          item.kind === "video" ? (
            <video
              src={src}
              controls
              autoPlay
              muted
              loop
              playsInline
              preload="metadata"
              poster={item.poster && poster ? poster : undefined}
              draggable
              onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [`${workspace}/${item.path}`])}
              title="Drag into the chat to attach"
            />
          ) : (
            <img
              src={src}
              alt={item.prompt}
              draggable
              onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [`${workspace}/${item.path}`])}
              title="Drag into the chat to attach"
            />
          )
        ) : (
          <div className="gallery-detail-nofile">{item.error ?? item.status}</div>
        )}
        <button type="button" className="gallery-detail-close" onClick={onClose} aria-label="Close details">
          <X size={13} />
        </button>
      </div>
      <div className="gallery-detail-body">
        <p className="gallery-detail-prompt">{item.prompt}</p>
        <div className="gallery-detail-meta">
          <span className="gallery-detail-model" title={item.model}>
            {item.model}
          </span>
          <span>{whenLabel(item.created_at)}</span>
          {item.cost_usd !== null && <span>{fmtUsd(item.cost_usd)}</span>}
          {item.width && item.height && (
            <span>
              {item.width}×{item.height}
            </span>
          )}
          <span className={`gallery-status ${item.status}`}>{item.status.replace("_", " ")}</span>
        </div>
        {params.length > 0 && (
          <div className="gallery-params">
            {params.map(([k, v]) => (
              <span key={k} className="gallery-param">
                <span className="gallery-param-key">{k}</span> {String(v)}
              </span>
            ))}
          </div>
        )}
        {item.refs.length > 0 && (
          <div className="gallery-refs">
            <span className="gallery-refs-label">refs</span>
            {item.refs.map((r) => (
              <RefThumb key={r} workspace={workspace} path={r} />
            ))}
          </div>
        )}
        {item.parent && (
          <div className="gallery-parent">
            from <code>{parent?.path ?? item.parent}</code>
          </div>
        )}
        <div className="gallery-actions">
          {item.path && (
            <button
              type="button"
              className="gallery-action"
              onClick={() => stageAttachment(`${workspace}/${item.path}`)}
              title="Attach to your next message as a reference"
            >
              <Paperclip size={13} /> Use as reference
            </button>
          )}
          {item.path && (
            <button type="button" className="gallery-action" onClick={() => openInViewer([item.path!])}>
              <FileCode2 size={13} /> Open in editor
            </button>
          )}
          {item.path && (
            <button type="button" className="gallery-action" onClick={() => revealInFiles(item.path!)}>
              <FolderTree size={13} /> Reveal in Files
            </button>
          )}
          <button type="button" className="gallery-action" onClick={copyPrompt}>
            {copied ? <Check size={13} /> : <Copy size={13} />} {copied ? "Copied" : "Copy prompt"}
          </button>
        </div>
      </div>
    </div>
  );
}

function RefThumb({ workspace, path }: { workspace: string; path: string }) {
  const src = useAssetSrc(workspace, path, 0);
  const name = path.split("/").pop() ?? path;
  if (!src) return <span className="gallery-ref-chip">{name}</span>;
  return path.match(/\.(png|jpe?g|webp|gif)$/i) ? (
    <img
      className="gallery-ref-thumb"
      src={src}
      alt={name}
      title={`${path} — drag into the chat to attach`}
      draggable
      onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [`${workspace}/${path}`])}
    />
  ) : (
    <span className="gallery-ref-chip" title={path}>
      {name}
    </span>
  );
}
