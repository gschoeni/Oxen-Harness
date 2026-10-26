// A video shown as a still: its poster (or first frame) under a play badge,
// for anywhere a video sits in a tile rather than a player — the composer
// tray, the gallery, a file grid. One look everywhere, so a video reads as a
// video at a glance without being a bare film icon.

import { useRef, useState, type DragEvent } from "react";
import { Film, Play } from "lucide-react";
import "./video.css";

/** WebKit paints no frame for `preload="metadata"` until something seeks, so
 *  a media fragment asks it to decode one just past the start (a frame at 0
 *  is often black on fade-ins). */
export function firstFrameSrc(src: string): string {
  return src.includes("#") ? src : `${src}#t=0.1`;
}

export function VideoThumb({
  src,
  poster,
  label,
  className,
  hoverPlay = false,
}: {
  src: string;
  poster?: string | null;
  label?: string;
  className?: string;
  /** Play muted while the pointer rests on it (gallery tiles). */
  hoverPlay?: boolean;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const [playing, setPlaying] = useState(false);
  // Tied to the source, so a rewritten file (a new cache-buster) gets
  // another try.
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  const failed = failedSrc === src;
  const classes = ["video-still", playing && "playing", className].filter(Boolean).join(" ");
  if (failed) {
    return (
      <span className={`${classes} failed`} role="img" aria-label={label ?? "video"}>
        <Film size={20} aria-hidden="true" />
      </span>
    );
  }
  return (
    <span
      className={classes}
      onMouseEnter={hoverPlay ? () => void ref.current?.play().catch(() => {}) : undefined}
      onMouseLeave={hoverPlay ? () => ref.current?.pause() : undefined}
    >
      <video
        ref={ref}
        src={firstFrameSrc(src)}
        poster={poster ?? undefined}
        muted
        loop={hoverPlay}
        playsInline
        preload="metadata"
        aria-label={label ?? "video"}
        draggable={false}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onError={() => setFailedSrc(src)}
      />
      <span className="video-still-play" aria-hidden="true">
        <Play />
      </span>
    </span>
  );
}

/** A thumbnail that becomes the native player, playing, once clicked: inline
 *  videos (chat attachments, generated clips) look like every other video
 *  until someone asks to watch one. */
export function InlineVideo({
  src,
  poster,
  label,
  className,
  title,
  onDragStart,
}: {
  src: string;
  poster?: string | null;
  label?: string;
  className?: string;
  title?: string;
  /** Makes it a drag source (attach the clip again by dropping it). */
  onDragStart?: (e: DragEvent) => void;
}) {
  const [open, setOpen] = useState(false);
  const drag = onDragStart ? { draggable: true, onDragStart } : {};
  if (open) {
    return (
      <video
        className={className}
        src={src}
        poster={poster ?? undefined}
        controls
        autoPlay
        playsInline
        aria-label={label ?? "video"}
        title={title}
        {...drag}
      />
    );
  }
  return (
    <button
      type="button"
      className={`video-still-button ${className ?? ""}`}
      onClick={() => setOpen(true)}
      aria-label={`Play ${label ?? "video"}`}
      title={title}
      {...drag}
    >
      <VideoThumb src={src} poster={poster} label={label} />
    </button>
  );
}
