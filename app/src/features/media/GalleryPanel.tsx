// The Gallery dock: the project's generated images and videos as a feed,
// newest first, with what's still rendering at the top. Fed by the project's
// `media://changed` events (the whole library each time) plus a cold load,
// and re-read when the watcher sees the output folder change (a file edited
// or deleted outside the app). The dock shows one thing at a time: the grid,
// or one generation in full (`GenerationDetail`) with the header turned into
// a back button and a prev/next stepper over the filtered feed. Arrow keys
// step, Escape returns to the grid, and the tile last looked at stays
// highlighted so the way back lands where you left.

import { useCallback, useEffect, useMemo, useState, type PointerEvent } from "react";
import { IconButton } from "../../components/ui";
import { ChevronLeft, ChevronRight, Images } from "lucide-react";
import { useStore } from "../../lib/store";
import type { MediaItem } from "../../lib/types";
import { isInFlight } from "../../lib/media";
import { useFsChangedUnder } from "../files/useFsChanged";
import { MediaTile } from "./MediaTile";
import { GenerationDetail } from "./GenerationDetail";
import "./media.css";

type Filter = "all" | "image" | "video";
const NONE: MediaItem[] = [];

export function GalleryPanel({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const sessionId = useStore((s) => s.session?.session_id);
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const items =
    useStore((s) => (s.session?.workspace ? s.media[s.session.workspace] : undefined)) ?? NONE;
  const refreshMedia = useStore((s) => s.refreshMedia);
  const mediaFocus = useStore((s) => s.mediaFocus);
  const contextFocus = useStore((s) =>
    s.session ? s.workContexts[s.session.session_id]?.current.id : undefined,
  );
  const clearMediaFocus = useStore((s) => s.clearMediaFocus);
  const cancelMedia = useStore((s) => s.cancelMedia);
  const [filter, setFilter] = useState<Filter>("all");
  const [thisChat, setThisChat] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [viewing, setViewing] = useState(false);
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

  useEffect(() => {
    if (contextFocus) setSelected(contextFocus);
    setViewing(!!contextFocus);
  }, [contextFocus]);

  // A chat card asked for one item: open it in full.
  useEffect(() => {
    if (!mediaFocus) return;
    setSelected(mediaFocus);
    setViewing(true);
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
  const detail = viewing && selected ? (items.find((i) => i.id === selected) ?? null) : null;
  const position = detail ? shown.findIndex((i) => i.id === detail.id) : -1;
  const open = useCallback(
    (id: string) => {
      setSelected(id);
      setViewing(true);
      if (sessionId) useStore.getState().openWorkView(sessionId, { view: "gallery", id });
    },
    [sessionId],
  );
  const back = useCallback(() => {
    setViewing(false);
    if (sessionId) useStore.getState().openWorkView(sessionId, { view: "gallery" });
  }, [sessionId]);
  const step = useCallback(
    (delta: number) => {
      if (position < 0) return;
      const next = shown[position + delta];
      if (next) open(next.id);
    },
    [position, shown, open],
  );

  // Keys work anywhere in the window while a generation is open, except in
  // a field the user is typing into.
  useEffect(() => {
    if (!detail) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      if (e.key === "ArrowLeft") step(-1);
      else if (e.key === "ArrowRight") step(1);
      else if (e.key === "Escape") back();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [detail, step, back]);

  const inFlight = items.filter(isInFlight).length;
  const uploading =
    useStore((s) => (workspace ? s.mediaUploads[workspace] : undefined))?.filter(
      (u) => u.status === "uploading" || u.status === "presigning",
    ).length ?? 0;

  if (!workspace) return null;
  if (detail) {
    return (
      <aside className="canvas gallery" aria-label="Gallery">
        {onResizeStart && <div className="canvas-resizer" onPointerDown={onResizeStart} />}
        <header className="canvas-head gallery-head">
          <IconButton
            type="button"
            className="sm"
            onClick={back}
            aria-label="Back to all generations"
            title="Back (Esc)"
          >
            <ChevronLeft size={15} />
          </IconButton>
          <span className="gallery-position">
            {position >= 0 ? `${position + 1} of ${shown.length}` : "1 generation"}
          </span>
          <div className="gallery-nav">
            <IconButton
              type="button"
              className="sm"
              onClick={() => step(-1)}
              disabled={position <= 0}
              aria-label="Previous generation"
              title="Previous (←)"
            >
              <ChevronLeft size={15} />
            </IconButton>
            <IconButton
              type="button"
              className="sm"
              onClick={() => step(1)}
              disabled={position < 0 || position >= shown.length - 1}
              aria-label="Next generation"
              title="Next (→)"
            >
              <ChevronRight size={15} />
            </IconButton>
          </div>
        </header>
        <GenerationDetail item={detail} workspace={workspace} bust={bust} onOpen={open} />
      </aside>
    );
  }
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
                onSelect={() => open(item.id)}
                onCancel={() => cancelMedia(item.id)}
              />
            ))}
          </div>
        </div>
      )}
    </aside>
  );
}
