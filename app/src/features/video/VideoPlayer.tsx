// The app's video player: a native <video> under our own transport bar, with
// frame-accurate stepping, scrubbing, save-frame, and a theater mode that
// takes over the whole app window.
//
// Theater mode moves the player, not a copy: the player renders into a host
// element we own, and that one element is re-parented between its slot in the
// dock and <body>. Moving a playing <video> keeps it playing, whereas a React
// portal switching containers would remount it and drop the playhead.

import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Film, Minimize2 } from "lucide-react";
import { IconButton } from "../../components/ui";
import { fsSaveDownload } from "../../lib/ipc";
import { DOWNLOAD_NOTE_MS } from "../media/useDownload";
import { frameFilename } from "./format";
import { useVideoFrameEngine } from "./useVideoFrameEngine";
import { captureElementFrame, extractFramePng } from "./videoFrames";
import { VideoControls } from "./VideoControls";
import "./video.css";

/** How long the cursor may rest before playing controls fade. */
const IDLE_HIDE_MS = 2500;

type SaveNote = { kind: "saved" | "failed"; text: string; title: string } | null;
type LoadError = "load" | "unsupported" | null;

/** Key the player by `src`: per-source state (frame index, playhead) resets
 *  with a remount. `path` names the file — for the theater header and the
 *  saved frame's name. */
export function VideoPlayer({ src, path }: { src: string; path: string }) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const slotRef = useRef<HTMLDivElement>(null);
  const [host] = useState(() => document.createElement("div"));
  const [theater, setTheater] = useState(false);
  const [aspect, setAspect] = useState(16 / 9);
  const [error, setError] = useState<LoadError>(null);
  const [hovering, setHovering] = useState(false);
  const [idle, setIdle] = useState(false);
  const [savingFrame, setSavingFrame] = useState(false);
  const [note, setNote] = useState<SaveNote>(null);
  const idleTimer = useRef<number | undefined>(undefined);
  const noteTimer = useRef<number | undefined>(undefined);

  const onMediaError = useCallback(() => {
    const code = videoRef.current?.error?.code;
    // Decoded fine but the codec isn't playable here (ProRes, FFV1, …):
    // retrying can't help, so say what it is.
    const unsupported =
      code === MediaError.MEDIA_ERR_DECODE || code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED;
    setError(unsupported ? "unsupported" : "load");
  }, []);
  const engine = useVideoFrameEngine({ videoRef, src, onMediaError });

  // Re-parent the one host element: into the dock slot normally, onto <body>
  // in theater mode. The class goes on before the move so the overlay
  // watchers (native webviews hide under it) see a theater the moment it
  // lands.
  useEffect(() => {
    host.className = theater ? "video-host video-theater" : "video-host";
    const parent = theater ? document.body : slotRef.current;
    if (parent && host.parentElement !== parent) parent.appendChild(host);
  }, [host, theater]);
  useEffect(() => () => host.remove(), [host]);

  const toggleTheater = useCallback(() => setTheater((t) => !t), []);

  // Theater owns Escape and the keyboard: capture phase beats the dock's and
  // dialogs' own Escape handlers, and focus moves in so arrows/space work
  // without a click first.
  useEffect(() => {
    if (!theater) return;
    rootRef.current?.focus({ preventScroll: true });
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setTheater(false);
    };
    window.addEventListener("keydown", onEscape, true);
    return () => window.removeEventListener("keydown", onEscape, true);
  }, [theater]);

  useEffect(
    () => () => {
      window.clearTimeout(idleTimer.current);
      window.clearTimeout(noteTimer.current);
    },
    [],
  );

  const wake = useCallback(() => {
    setIdle(false);
    window.clearTimeout(idleTimer.current);
    idleTimer.current = window.setTimeout(() => setIdle(true), IDLE_HIDE_MS);
  }, []);

  const saveFrame = useCallback(async () => {
    const video = videoRef.current;
    if (!video) return;
    engine.pause();
    setSavingFrame(true);
    window.clearTimeout(noteTimer.current);
    setNote(null);
    const { frame, time } = engine.getFrame();
    const name = frameFilename(path, frame);
    try {
      // Exact decoded pixels first; the element's own pixels if this webview
      // can't decode the track itself.
      const exact = await extractFramePng(src, time).catch((e: unknown) => {
        console.warn("exact frame decode failed; grabbing the displayed frame", e);
        return null;
      });
      const blob = exact ?? (await captureElementFrame(video));
      if (!blob) throw new Error("this video's frames can't be read here");
      const saved = await fsSaveDownload(name, new Uint8Array(await blob.arrayBuffer()));
      setNote({ kind: "saved", text: `Saved ${name} to Downloads`, title: saved });
    } catch (e) {
      setNote({ kind: "failed", text: `Couldn't save frame ${frame + 1}`, title: String(e) });
    } finally {
      setSavingFrame(false);
      noteTimer.current = window.setTimeout(() => setNote(null), DOWNLOAD_NOTE_MS);
    }
  }, [engine, path, src]);

  const onKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      // Leave OS / app shortcuts alone; Shift is ours (±10 frames).
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target as HTMLElement;
      // The volume slider owns its arrows; buttons own Space/Enter.
      if (target.tagName === "INPUT") return;
      const onButton = !!target.closest("button");
      const handled = (() => {
        switch (event.key) {
          case " ":
          case "k":
            if (onButton && event.key === " ") return false;
            engine.togglePlay();
            return true;
          case "ArrowLeft":
            engine.stepFrame(event.shiftKey ? -10 : -1);
            return true;
          case "ArrowRight":
            engine.stepFrame(event.shiftKey ? 10 : 1);
            return true;
          case "Home":
            engine.seekToFrame(0);
            return true;
          case "End":
            engine.seekToFrame(engine.totalFrames - 1);
            return true;
          case "f":
            toggleTheater();
            return true;
          case "m":
            engine.setMuted(!engine.muted);
            return true;
          default:
            return false;
        }
      })();
      if (handled) {
        event.preventDefault();
        wake();
      }
    },
    [engine, toggleTheater, wake],
  );

  const name = path.split("/").pop() ?? path;
  const controlsVisible = !engine.isPlaying || (hovering && !idle);

  const player = (
    <div
      ref={rootRef}
      className={`video-player${theater ? " theater" : ""}${idle && engine.isPlaying ? " idle" : ""}`}
      tabIndex={-1}
      onKeyDown={onKeyDown}
      onPointerMove={() => {
        setHovering(true);
        wake();
      }}
      onPointerLeave={() => setHovering(false)}
    >
      {theater && (
        <header className="video-theater-head">
          <span className="video-theater-name" title={path}>
            <Film size={14} aria-hidden="true" />
            {name}
          </span>
          <IconButton size="sm" onClick={toggleTheater} aria-label="Exit theater mode" title="Exit (esc)">
            <Minimize2 size={15} />
          </IconButton>
        </header>
      )}
      <div className="video-stage">
        <div
          className="video-frame"
          style={{ "--video-aspect": aspect } as CSSProperties}
          onClick={() => {
            rootRef.current?.focus({ preventScroll: true });
            engine.togglePlay();
          }}
          onDoubleClick={toggleTheater}
        >
          <video
            ref={videoRef}
            src={src}
            // Lets "save frame" read the element's pixels without tainting
            // the canvas (the asset protocol answers with CORS headers).
            crossOrigin="anonymous"
            playsInline
            preload="auto"
            aria-label={name}
            onLoadedMetadata={(e) => {
              const v = e.currentTarget;
              if (v.videoWidth > 0 && v.videoHeight > 0) setAspect(v.videoWidth / v.videoHeight);
            }}
          />
          {error ? (
            <div className="video-error" role="alert">
              {error === "unsupported"
                ? "This video's format can't be played in the app. Open it in a local player instead."
                : "This video couldn't be loaded."}
            </div>
          ) : (
            <VideoControls
              engine={engine}
              visible={controlsVisible}
              theater={theater}
              onToggleTheater={toggleTheater}
              onSaveFrame={() => void saveFrame()}
              savingFrame={savingFrame}
            />
          )}
          {note && (
            <div className={`video-note ${note.kind}`} role="status" title={note.title}>
              {note.text}
            </div>
          )}
        </div>
      </div>
    </div>
  );

  return (
    <div ref={slotRef} className="video-slot">
      {createPortal(player, host)}
    </div>
  );
}
