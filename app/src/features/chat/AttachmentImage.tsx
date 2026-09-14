// Renders a media attachment by reference. The ref is either an absolute path
// (a freshly picked file in the composer) or a path relative to the session's
// workspace (how persisted attachments are stored). The backend resolves it to
// an absolute path and the webview loads the bytes through the asset protocol
// — the same route the Files dock's media view takes — so an image costs the
// app no base64 copy: not built in Rust, not shipped over IPC, not retained in
// the JS heap for as long as the bubble is mounted. The webview's own image
// cache manages the decoded bitmap and can evict it under pressure.
//
// Videos and audio (references for video generation) render as native players
// over the same asset route.

import { useEffect, useState, type DragEvent } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { attachmentPath } from "../../lib/ipc";
import { isAudioPath, isVideoPath } from "../../lib/attachments";
import { useStore } from "../../lib/store";
import { setDragPaths } from "../files/dnd";

export function AttachmentImage({
  src,
  alt,
  className,
}: {
  src: string;
  alt?: string;
  className?: string;
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
    return <video className={className} src={url} controls preload="metadata" aria-label={alt ?? "video attachment"} {...drag} />;
  }
  if (isAudioPath(src)) {
    return <audio className={className} src={url} controls preload="metadata" aria-label={alt ?? "audio attachment"} {...drag} />;
  }
  return <img className={className} src={url} alt={alt ?? "attachment"} {...drag} />;
}
