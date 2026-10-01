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

import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { ArrowLeftRight, Pause, Play } from "lucide-react";
import { IconButton } from "../../components/ui";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import type { MediaItem, MediaSource } from "../../lib/types";
import { useAssetSrc } from "../files/useAssetSrc";

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
  const [a, setA] = useState(options[0].path);
  const [b, setB] = useState(options[options.length - 1].path);
  const [filling, setFilling] = useState<Side>("a");
  const [split, setSplit] = useState(50);
  const [playing, setPlaying] = useState(true);
  const [frame, setFrame] = useState(aspect);
  const stage = useRef<HTMLDivElement>(null);
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
    setSplit(Math.round(Math.max(0, Math.min(100, ((clientX - box.left) / box.width) * 100))));
  }

  function onPointerDown(e: PointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    moveTo(e.clientX);
  }

  function onPointerMove(e: PointerEvent<HTMLDivElement>) {
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) moveTo(e.clientX);
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
    setSplit(Math.max(0, Math.min(100, next)));
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

  return (
    <div className="compare">
      <div
        ref={stage}
        className="compare-stage"
        style={{ aspectRatio: frame ?? 16 / 9 }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
      >
        <Layer
          side="a"
          option={sideA}
          workspace={workspace}
          bust={bust}
          videoRef={(el) => (videos.current.a = el)}
        />
        <Layer
          side="b"
          option={sideB}
          workspace={workspace}
          bust={bust}
          clip={split}
          videoRef={(el) => (videos.current.b = el)}
          onTimeUpdate={keepInStep}
          // The frame takes side B's shape (the output, unless they were swapped).
          onSize={(w, h) => w > 0 && h > 0 && setFrame(w / h)}
        />
        <span className="compare-tag a" title={sideA.title}>
          <b>A</b> {sideA.title}
        </span>
        <span className="compare-tag b" title={sideB.title}>
          <b>B</b> {sideB.title}
        </span>
        <div
          className="compare-divider"
          style={{ left: `${split}%` }}
          role="slider"
          tabIndex={0}
          aria-label="Comparison divider"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={split}
          aria-valuetext={`${split}% side A, ${100 - split}% side B`}
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
                    <Thumb option={o} workspace={workspace} bust={bust} />
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

/** One side of the frame. Side B is drawn over A and clipped at the divider. */
function Layer({
  side,
  option,
  workspace,
  bust,
  clip,
  videoRef,
  onTimeUpdate,
  onSize,
}: {
  side: Side;
  option: CompareOption;
  workspace: string;
  bust: number;
  clip?: number;
  videoRef: (el: HTMLVideoElement | null) => void;
  onTimeUpdate?: () => void;
  onSize?: (width: number, height: number) => void;
}) {
  const src = useAssetSrc(workspace, option.path, bust);
  if (!src) return null;
  const shared = {
    className: `compare-layer ${side}`,
    style: clip === undefined ? undefined : { clipPath: `inset(0 0 0 ${clip}%)` },
    draggable: false,
  };
  return option.kind === "video" ? (
    <video
      {...shared}
      ref={videoRef}
      src={src}
      aria-label={`Side ${side.toUpperCase()}: ${option.title}`}
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
      {...shared}
      src={src}
      alt={`Side ${side.toUpperCase()}: ${option.title}`}
      onLoad={(e) => onSize?.(e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)}
    />
  );
}

function Thumb({ option, workspace, bust }: { option: CompareOption; workspace: string; bust: number }) {
  const src = useAssetSrc(workspace, option.path, bust);
  if (!src) return <span className="compare-thumb blank" aria-hidden="true" />;
  return option.kind === "video" ? (
    <video className="compare-thumb" src={src} muted playsInline preload="metadata" />
  ) : (
    <img className="compare-thumb" src={src} alt="" draggable={false} />
  );
}
