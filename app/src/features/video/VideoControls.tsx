// The transport bar floating over the bottom of the video: play, frame
// stepping, the scrub timeline, a timecode + frame readout, loop, volume,
// save-frame, and theater mode. Re-renders at video fps through the engine's
// frame subscription; nothing above it does.

import { memo, useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import {
  ImageDown,
  Maximize2,
  Minimize2,
  Pause,
  Play,
  Repeat,
  StepBack,
  StepForward,
  Volume2,
  VolumeX,
} from "lucide-react";
import { IconButton, Spinner } from "../../components/ui";
import { formatFrameCounter, formatTimecode } from "./format";
import { usePointerDrag } from "./usePointerDrag";
import type { VideoFrameEngine } from "./useVideoFrameEngine";

export const VideoControls = memo(function VideoControls({
  engine,
  visible,
  theater,
  onToggleTheater,
  onSaveFrame,
  savingFrame,
}: {
  engine: VideoFrameEngine;
  /** Shown while hovered / paused; scrubbing and keyboard focus also hold it. */
  visible: boolean;
  theater: boolean;
  onToggleTheater: () => void;
  onSaveFrame: () => void;
  savingFrame: boolean;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const engineRef = useRef(engine);
  useEffect(() => {
    engineRef.current = engine;
  }, [engine]);

  const { isDragging: scrubbing, beginDrag } = usePointerDrag({
    targetRef: trackRef,
    onFraction: (fraction) => engineRef.current.scrubToFraction(fraction),
    onDragStart: () => engineRef.current.beginScrub(),
    onDragEnd: () => engineRef.current.endScrub(),
  });
  const onTrackPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (beginDrag(event)) event.preventDefault();
    },
    [beginDrag],
  );

  const { frame, time } = useSyncExternalStore(engine.subscribeFrames, engine.getFrame, engine.getFrame);
  const total = engine.totalFrames;
  const fraction = engine.duration > 0 ? Math.min(1, time / engine.duration) : 0;
  const timecode = formatTimecode(time);
  const counter =
    total > 0 ? formatFrameCounter(Math.min(frame, total - 1), total, engine.frameIndexExact) : null;
  const ready = engine.isReady;

  return (
    <div
      className={`video-controls${visible || scrubbing ? " shown" : ""}`}
      // Clicks on the bar must not reach the stage's click-to-play.
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <IconButton
        size="sm"
        onClick={engine.togglePlay}
        disabled={!ready}
        aria-label={engine.isPlaying ? "Pause" : "Play"}
        title={engine.isPlaying ? "Pause (space)" : "Play (space)"}
      >
        {engine.isPlaying ? <Pause size={15} /> : <Play size={15} />}
      </IconButton>
      <IconButton
        size="sm"
        onClick={() => engine.stepFrame(-1)}
        disabled={!ready}
        aria-label="Previous frame"
        title="Previous frame (←, ⇧← for 10)"
      >
        <StepBack size={15} />
      </IconButton>
      <IconButton
        size="sm"
        onClick={() => engine.stepFrame(1)}
        disabled={!ready}
        aria-label="Next frame"
        title="Next frame (→, ⇧→ for 10)"
      >
        <StepForward size={15} />
      </IconButton>
      <div
        ref={trackRef}
        className={`video-timeline${scrubbing ? " scrubbing" : ""}`}
        role="slider"
        tabIndex={0}
        aria-label="Video timeline"
        aria-valuemin={0}
        aria-valuemax={engine.duration}
        aria-valuenow={time}
        aria-valuetext={counter ? `${timecode}, frame ${counter}` : timecode}
        data-video-timeline="true"
        onPointerDown={onTrackPointerDown}
      >
        <div className="video-rail">
          <div className="video-fill" style={{ width: `${fraction * 100}%` }} />
          <div className="video-thumb" style={{ left: `${fraction * 100}%` }} />
        </div>
      </div>
      <span className="video-readout" aria-hidden="true">
        <span>{timecode}</span>
        {counter && <span className="video-frames">{counter}</span>}
      </span>
      <IconButton
        size="sm"
        className="video-loop"
        active={engine.loop}
        onClick={() => engine.setLoop(!engine.loop)}
        aria-label="Loop"
        aria-pressed={engine.loop}
        title="Loop"
      >
        <Repeat size={15} />
      </IconButton>
      <div className="video-volume">
        <IconButton
          size="sm"
          onClick={() => engine.setMuted(!engine.muted)}
          aria-label={engine.muted ? "Unmute" : "Mute"}
          title={engine.muted ? "Unmute (m)" : "Mute (m)"}
        >
          {engine.muted || engine.volume === 0 ? <VolumeX size={15} /> : <Volume2 size={15} />}
        </IconButton>
        <input
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={engine.muted ? 0 : engine.volume}
          onChange={(e) => engine.setVolume(Number(e.target.value))}
          aria-label="Volume"
        />
      </div>
      <IconButton
        size="sm"
        onClick={onSaveFrame}
        disabled={!ready || savingFrame}
        aria-label="Save frame as PNG"
        title="Save this frame to Downloads as PNG"
      >
        {savingFrame ? <Spinner /> : <ImageDown size={15} />}
      </IconButton>
      <IconButton
        size="sm"
        onClick={onToggleTheater}
        aria-label={theater ? "Exit theater mode" : "Theater mode"}
        title={theater ? "Exit theater mode (f, esc)" : "Theater mode (f)"}
      >
        {theater ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
      </IconButton>
    </div>
  );
});
