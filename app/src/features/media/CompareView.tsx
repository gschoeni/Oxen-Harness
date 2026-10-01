// A/B comparison for one generation: two of its pictures (or clips) stacked
// in the same frame with a divider you drag across — side A to the left of
// it, side B to the right. It opens on the obvious question, "what did the
// model do to my input": the first reference on A, the output on B.
//
// With several references the two sides are picked the way you'd say it out
// loud: choose a side (A or B), then click the picture that goes there. Every
// candidate wears the badge of the side it is on, the two sides can never
// hold the same picture (picking the other side's swaps them), and ⇄ flips
// them. Two videos play in step, so the divider compares the same moment.
//
// The frame is built to hold still, because any blink reads as "the two
// pictures are flickering":
// - Every candidate's URL is resolved once, up front, and pinned for the life
//   of the comparison. Changing a side is then a plain `src` change (the old
//   picture stays up until the new one is ready) instead of an async gap, and
//   a file landing in the output folder mid-compare (the gallery's cache
//   bust) does not reload what is on screen.
// - The divider never restyles a picture. Each side is a wrapper; side B's
//   wrapper is what gets clipped, and the pictures inside are composited
//   layers decoded synchronously — a big PNG that WebKit re-decoded on every
//   repaint would blank for a frame and show side A through it.
// - Dragging selects nothing. A selection paints the accent tint over the
//   pictures and flips as the pointer moves; the press is cancelled and the
//   frame is unselectable (with WebKit's prefixed property — it ignores the
//   standard one).
// - The frame takes the output's shape, once. It does not follow whichever
//   picture happens to be on side B.

import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from "react";
import { ArrowLeftRight, Pause, Play } from "lucide-react";
import { IconButton } from "../../components/ui";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import type { MediaItem, MediaSource } from "../../lib/types";
import { convertFileSrc } from "@tauri-apps/api/core";
import { fsAssetPath } from "../../lib/ipc";

/** One thing that can sit on a side of the comparison. */
export interface CompareOption {
  /** Its project-relative path, which is also its identity. */
  path: string;
  kind: "image" | "video";
  title: string;
  /** What it is to this generation: "reference", "output", … */
  detail: string;
  output: boolean;
}

type Side = "a" | "b";

const fileName = (path: string) => path.split("/").pop() ?? path;
const visualKind = (path: string) => (isVideoPath(path) ? "video" : isImagePath(path) ? "image" : null);

/** The pictures and clips a generation can be compared across: each visual
 *  input in request order (references, then a parent they don't already
 *  cover), and the output last. Empty unless there is an output and at least
 *  one input to hold it against. */
export function compareOptions(item: MediaItem, inputs: MediaSource[]): CompareOption[] {
  const outputKind = item.path ? visualKind(item.path) : null;
  if (!item.path || !outputKind) return [];
  const options: CompareOption[] = [];
  const add = (path: string, title: string, detail: string) => {
    const kind = visualKind(path);
    if (kind && path !== item.path && !options.some((o) => o.path === path)) {
      options.push({ path, kind, title, detail, output: false });
    }
  };
  for (const s of inputs) {
    const name = fileName(s.source);
    add(s.path, s.label ? `${s.label} ${name}` : name, s.origin === "generation" ? "earlier generation" : "reference");
  }
  if (item.parent && !inputs.some((s) => s.source === item.parent)) {
    add(item.parent, fileName(item.parent), "earlier file it varies");
  }
  if (options.length === 0) return [];
  options.push({ path: item.path, kind: outputKind, title: fileName(item.path), detail: "output", output: true });
  return options;
}

/** Asset URLs for every candidate, resolved once through the same canonical
 *  boundary check as `useAssetSrc` (a path outside the workspace yields
 *  nothing) and never re-keyed while the comparison is open. */
function useAssetSrcs(workspace: string, paths: string[], bust: number): Record<string, string> {
  const [srcs, setSrcs] = useState<Record<string, string>>({});
  // Pinned at mount: see the note on holding still, above.
  const version = useRef(bust).current;
  const key = paths.join("\n");
  useEffect(() => {
    let stale = false;
    setSrcs({});
    for (const path of key ? key.split("\n") : []) {
      fsAssetPath(workspace, path)
        .then((real) => {
          if (stale) return;
          const src = convertFileSrc(real) + (version ? `?v=${version}` : "");
          setSrcs((prev) => (prev[path] === src ? prev : { ...prev, [path]: src }));
        })
        .catch(() => {
          /* outside the boundary (or gone): that side stays empty */
        });
    }
    return () => {
      stale = true;
    };
  }, [workspace, key, version]);
  return srcs;
}

/** How far an arrow key moves the divider, in percent. */
const KEY_STEP = 2;
/** How far two clips may drift apart before the follower is pulled back. */
const MAX_DRIFT_SECS = 0.2;

export function CompareView({
  workspace,
  options,
  bust,
  aspect,
}: {
  workspace: string;
  /** From `compareOptions`: at least one input, the output last. */
  options: CompareOption[];
  bust: number;
  /** The output's width / height, when the manifest knows it. */
  aspect?: number;
}) {
  const srcs = useAssetSrcs(
    workspace,
    useMemo(() => options.map((o) => o.path), [options]),
    bust,
  );
  const [a, setA] = useState(options[0].path);
  const [b, setB] = useState(options[options.length - 1].path);
  const [filling, setFilling] = useState<Side>("a");
  const [split, setSplit] = useState(50);
  const [playing, setPlaying] = useState(true);
  // The output's shape: from the manifest, else measured when it first loads.
  const [frame, setFrame] = useState(aspect);
  const measure = (option: CompareOption) =>
    option.output && frame === undefined
      ? (w: number, h: number) => {
          if (w > 0 && h > 0) setFrame(w / h);
        }
      : undefined;
  const stage = useRef<HTMLDivElement>(null);
  const divider = useRef<HTMLDivElement>(null);
  const videos = useRef<Partial<Record<Side, HTMLVideoElement | null>>>({});

  const byPath = useMemo(() => new Map(options.map((o) => [o.path, o])), [options]);
  const sideA = byPath.get(a) ?? options[0];
  const sideB = byPath.get(b) ?? options[options.length - 1];
  const hasVideo = sideA.kind === "video" || sideB.kind === "video";

  function choose(path: string) {
    const [mine, setMine, other, setOther] = filling === "a" ? [a, setA, b, setB] : [b, setB, a, setA];
    if (path === mine) return;
    // Taking the other side's picture trades places with it.
    if (path === other) setOther(mine);
    setMine(path);
  }

  function swap() {
    setA(b);
    setB(a);
  }

  function moveTo(clientX: number) {
    const box = stage.current?.getBoundingClientRect();
    if (!box || box.width === 0) return;
    setSplit(Math.max(0, Math.min(100, ((clientX - box.left) / box.width) * 100)));
  }

  function onPointerDown(e: PointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    // The press is a drag of the divider and nothing else: not the start of
    // a text selection, not a native image drag. (CSS says the same; this
    // holds even where it is not honored.) A selection left over from
    // elsewhere would otherwise stretch to follow the pointer.
    e.preventDefault();
    window.getSelection?.()?.removeAllRanges();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    // Cancelling the press also cancels click-to-focus; the arrow keys
    // should still work after a drag.
    divider.current?.focus({ preventScroll: true });
    moveTo(e.clientX);
  }

  function onPointerMove(e: PointerEvent<HTMLDivElement>) {
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) moveTo(e.clientX);
  }

  function onPointerEnd(e: PointerEvent<HTMLDivElement>) {
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture?.(e.pointerId);
  }

  function onKeyDown(e: KeyboardEvent) {
    const next =
      e.key === "ArrowLeft"
        ? split - KEY_STEP
        : e.key === "ArrowRight"
          ? split + KEY_STEP
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? 100
              : null;
    if (next === null) return;
    // The arrows belong to the divider while it has focus, not to the
    // gallery's previous/next.
    e.preventDefault();
    e.stopPropagation();
    setSplit(Math.max(0, Math.min(100, Math.round(next))));
  }

  useEffect(() => {
    for (const video of Object.values(videos.current)) {
      if (!video) continue;
      if (playing) void video.play?.()?.catch?.(() => {});
      else video.pause?.();
    }
  }, [playing, a, b]);

  /** Two clips stay on the same moment: B leads, A is pulled back when it drifts. */
  function keepInStep() {
    const lead = videos.current.b;
    const follow = videos.current.a;
    if (!lead || !follow) return;
    if (Math.abs(follow.currentTime - lead.currentTime) > MAX_DRIFT_SECS) follow.currentTime = lead.currentTime;
  }

  const shape = frame ?? 16 / 9;
  return (
    <div className="compare">
      <div
        ref={stage}
        className="compare-stage"
        style={{ aspectRatio: shape, "--compare-aspect": shape } as CSSProperties}
        onPointerDown={onPointerDown}
        // The compatibility mousedown is where WebKit starts a selection.
        onMouseDown={(e) => e.preventDefault()}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerEnd}
        onPointerCancel={onPointerEnd}
      >
        <div className="compare-side a">
          <Layer
            side="a"
            option={sideA}
            src={srcs[sideA.path]}
            videoRef={(el) => (videos.current.a = el)}
            onSize={measure(sideA)}
          />
        </div>
        {/* Drawn over A and clipped at the divider: B is what's right of it. */}
        <div className="compare-side b" style={{ clipPath: `inset(0 0 0 ${split}%)` }}>
          <Layer
            side="b"
            option={sideB}
            src={srcs[sideB.path]}
            videoRef={(el) => (videos.current.b = el)}
            onTimeUpdate={keepInStep}
            onSize={measure(sideB)}
          />
        </div>
        <span className="compare-tag a" title={sideA.title}>
          <b>A</b> {sideA.title}
        </span>
        <span className="compare-tag b" title={sideB.title}>
          <b>B</b> {sideB.title}
        </span>
        <div
          ref={divider}
          className="compare-divider"
          style={{ left: `${split}%` }}
          role="slider"
          tabIndex={0}
          aria-label="Comparison divider"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(split)}
          aria-valuetext={`${Math.round(split)}% side A, ${100 - Math.round(split)}% side B`}
          onKeyDown={onKeyDown}
        >
          <span className="compare-grip" aria-hidden="true">
            <ArrowLeftRight size={13} />
          </span>
        </div>
      </div>

      <div className="compare-controls">
        <div className="compare-slots" role="radiogroup" aria-label="Side to change">
          <Slot side="a" option={sideA} active={filling === "a"} pickable={options.length > 2} onPick={() => setFilling("a")} />
          <IconButton type="button" size="sm" onClick={swap} aria-label="Swap sides" title="Swap sides">
            <ArrowLeftRight size={14} />
          </IconButton>
          <Slot side="b" option={sideB} active={filling === "b"} pickable={options.length > 2} onPick={() => setFilling("b")} />
          {hasVideo && (
            <IconButton
              type="button"
              size="sm"
              onClick={() => setPlaying((p) => !p)}
              aria-label={playing ? "Pause" : "Play"}
              title={playing ? "Pause" : "Play"}
            >
              {playing ? <Pause size={14} /> : <Play size={14} />}
            </IconButton>
          )}
        </div>
        {/* One input and one output leave nothing to choose. */}
        {options.length > 2 && (
          <>
            <p className="compare-hint">
              Pick what goes on side <b>{filling.toUpperCase()}</b>
            </p>
            <div className="compare-options" role="group" aria-label={`Choices for side ${filling.toUpperCase()}`}>
              {options.map((o) => {
                const on: Side | null = o.path === a ? "a" : o.path === b ? "b" : null;
                return (
                  <button
                    key={o.path}
                    type="button"
                    className={`compare-option ${on ? `on ${on}` : ""}`}
                    aria-pressed={on === filling}
                    aria-label={`${o.title} (${o.detail})${on ? `, on side ${on.toUpperCase()}` : ""}`}
                    title={`${o.title} — ${o.detail}`}
                    onClick={() => choose(o.path)}
                  >
                    <Thumb option={o} src={srcs[o.path]} />
                    {on && <span className={`compare-badge ${on}`}>{on.toUpperCase()}</span>}
                    <span className="compare-option-name">{o.output ? "Output" : o.title}</span>
                  </button>
                );
              })}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** One side's header: its badge and what is on it. With more than two
 *  candidates it is also the switch for which side the next pick fills. */
function Slot({
  side,
  option,
  active,
  pickable,
  onPick,
}: {
  side: Side;
  option: CompareOption;
  active: boolean;
  pickable: boolean;
  onPick: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      aria-label={`Side ${side.toUpperCase()}: ${option.title}`}
      className={`compare-slot ${pickable && active ? "active" : ""}`}
      disabled={!pickable}
      onClick={onPick}
      title={pickable ? `Change side ${side.toUpperCase()}` : option.title}
    >
      <span className={`compare-badge ${side}`}>{side.toUpperCase()}</span>
      <span className="compare-slot-text">
        <span className="compare-slot-name">{option.output ? "Output" : option.title}</span>
        <span className="compare-slot-detail">{option.detail}</span>
      </span>
    </button>
  );
}

/** The picture or clip on one side. The element is reused when the side's
 *  choice changes, so the old picture stays up until the new one has loaded. */
function Layer({
  side,
  option,
  src,
  videoRef,
  onTimeUpdate,
  onSize,
}: {
  side: Side;
  option: CompareOption;
  src: string | undefined;
  videoRef: (el: HTMLVideoElement | null) => void;
  onTimeUpdate?: () => void;
  onSize?: (width: number, height: number) => void;
}) {
  if (!src) return null;
  return option.kind === "video" ? (
    <video
      className="compare-layer"
      ref={videoRef}
      src={src}
      aria-label={`Side ${side.toUpperCase()}: ${option.title}`}
      draggable={false}
      autoPlay
      muted
      loop
      playsInline
      preload="auto"
      onTimeUpdate={onTimeUpdate}
      onLoadedMetadata={(e) => onSize?.(e.currentTarget.videoWidth, e.currentTarget.videoHeight)}
    />
  ) : (
    <img
      className="compare-layer"
      src={src}
      alt={`Side ${side.toUpperCase()}: ${option.title}`}
      draggable={false}
      decoding="sync"
      onLoad={(e) => onSize?.(e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)}
    />
  );
}

function Thumb({ option, src }: { option: CompareOption; src: string | undefined }) {
  if (!src) return <span className="compare-thumb blank" aria-hidden="true" />;
  return option.kind === "video" ? (
    <video className="compare-thumb" src={src} muted playsInline preload="metadata" />
  ) : (
    <img className="compare-thumb" src={src} alt="" draggable={false} />
  );
}
