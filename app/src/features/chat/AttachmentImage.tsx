// Renders a media attachment by reference. The ref is either an absolute path
// (a freshly picked file in the composer) or a path relative to the session's
// workspace (how persisted attachments are stored). The backend resolves it to
// an absolute path and the webview loads the bytes through the asset protocol
// — the same route the Files dock's media view takes — so an image costs the
// app no base64 copy: not built in Rust, not shipped over IPC, not retained in
// the JS heap for as long as the bubble is mounted. The webview's own image
// cache manages the decoded bitmap and can evict it under pressure.
//
// Videos (references for video generation) render as a thumbnail with a play
// badge — a click turns it into the native player, or, in a tray tile, it
// stays a thumbnail. Audio renders as a native player over the same route.

import { useEffect, useState, type DragEvent } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { attachmentPath } from "../../lib/ipc";
import { isAudioPath, isVideoPath } from "../../lib/attachments";
import { useStore } from "../../lib/store";
import { setDragPaths } from "../files/dnd";
import { InlineVideo, VideoThumb } from "../video/VideoThumb";

export function AttachmentImage({
  src,
  alt,
  className,
  thumbnail = false,
}: {
  src: string;
  alt?: string;
  className?: string;
  /** A video stays a still (a composer tray tile) rather than becoming a
   *  player when clicked. */
  thumbnail?: boolean;
}) {
  const session = useStore((s) => s.session?.session_id);
  const [abs, setAbs] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    setAbs(null);
    setFailed(false);
    // Session-relative refs need the session to resolve; an absolute ref
    // resolves the same in any session (the backend just checks it exists).
    attachmentPath(src, session)
      .then((path) => alive && setAbs(path))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [src, session]);

  if (failed) return null;
  if (!abs) return <span className={`attach-img-loading ${className ?? ""}`} aria-hidden />;
  const url = convertFileSrc(abs);
  // Once the path is known the element is a drag source: dragging a picture
  // from an earlier message into the composer attaches it again.
  const drag = {
    draggable: true,
    onDragStart: (e: DragEvent) => setDragPaths(e.dataTransfer, [abs]),
    title: "Drag into the chat to attach",
  };
  if (isVideoPath(src)) {
    const label = alt ?? "video attachment";
    return thumbnail ? (
      <VideoThumb src={url} label={label} className={className} />
    ) : (
      <InlineVideo src={url} label={label} className={className} title={drag.title} onDragStart={drag.onDragStart} />
    );
  }
  if (isAudioPath(src)) {
    return <audio className={className} src={url} controls preload="metadata" aria-label={alt ?? "audio attachment"} {...drag} />;
  }
  return <img className={className} src={url} alt={alt ?? "attachment"} {...drag} />;
}
