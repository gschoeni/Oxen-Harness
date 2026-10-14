// Frame-accurate playback over a native <video>. Tracks the displayed frame
// with requestVideoFrameCallback, steps by seeking to mid-frame timestamps
// and confirming where the seek landed, and scrubs without losing whether
// the clip was playing. Ported from oxen hub's player (minus its A/B
// follower video); the frame grid comes from the demuxed container when
// mediabunny can read it, and is estimated from playback otherwise.

import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { loadVideoFrameIndex } from "./videoFrames";

export interface VideoFrameState {
  /** 0-based index of the displayed frame. */
  frame: number;
  /** Presentation time of the last confirmed displayed frame. */
  time: number;
}

export interface VideoFrameEngine {
  isReady: boolean;
  isPlaying: boolean;
  duration: number;
  /** Per-frame state changes at video fps, so it lives outside React state:
   *  subscribe with useSyncExternalStore instead of re-rendering the player. */
  getFrame: () => VideoFrameState;
  subscribeFrames: (listener: () => void) => () => void;
  totalFrames: number;
  /** True when real per-frame timestamps were read from the file. */
  frameIndexExact: boolean;
  loop: boolean;
  setLoop: (value: boolean) => void;
  muted: boolean;
  setMuted: (value: boolean) => void;
  volume: number;
  setVolume: (value: number) => void;
  play: () => void;
  pause: () => void;
  togglePlay: () => void;
  stepFrame: (delta: number) => void;
  seekToFrame: (frame: number) => void;
  scrubToFraction: (fraction: number) => void;
  beginScrub: () => void;
  endScrub: () => void;
}

const DEFAULT_FPS = 30;
const PREFS_KEY = "oxen-harness.video-playback";

interface PlaybackPrefs {
  muted: boolean;
  volume: number;
}

/** Mute/volume carry over from the last clip the user adjusted. A
 *  per-viewer convenience: unreadable storage just means the defaults. */
function readPrefs(): PlaybackPrefs | null {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<PlaybackPrefs>;
    if (typeof parsed.muted !== "boolean" || typeof parsed.volume !== "number") return null;
    return { muted: parsed.muted, volume: clamp(parsed.volume, 0, 1) };
  } catch {
    return null;
  }
}

function writePrefs(prefs: PlaybackPrefs) {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {
    /* storage unavailable: the setting just won't carry over */
  }
}

const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(max, value));

function swallowPlayRejection(error: unknown) {
  // A pause() racing play(), or autoplay policy, rejects play(); both are
  // expected. Anything else is worth seeing in the console.
  if (error instanceof DOMException && (error.name === "AbortError" || error.name === "NotAllowedError")) {
    return;
  }
  console.error("video play failed", error);
}

/** Last index with timestamps[i] <= time + eps. */
function frameIndexForTime(timestamps: number[], time: number, eps: number): number {
  let lo = 0;
  let hi = timestamps.length - 1;
  let result = 0;
  const target = time + eps;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (timestamps[mid] <= target) {
      result = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return result;
}

function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[sorted.length >> 1];
}

export function useVideoFrameEngine({
  videoRef,
  src,
  autoPlay = false,
  onMediaError,
}: {
  videoRef: RefObject<HTMLVideoElement | null>;
  /** Callers key the player by this: per-source state resets with it. */
  src: string;
  autoPlay?: boolean;
  onMediaError?: () => void;
}): VideoFrameEngine {
  const [isReady, setIsReady] = useState(false);
  const [isPlaying, setIsPlaying] = useState(false);
  const [duration, setDuration] = useState(0);
  const [fps, setFps] = useState(DEFAULT_FPS);
  // The real frame count once the container is indexed; null on the estimate.
  const [exactFrameCount, setExactFrameCount] = useState<number | null>(null);
  const [loop, setLoopState] = useState(false);
  const [muted, setMutedState] = useState(() => readPrefs()?.muted ?? false);
  const [volume, setVolumeState] = useState(() => readPrefs()?.volume ?? 1);

  const timestampsRef = useRef<number[] | null>(null);
  const fpsRef = useRef(DEFAULT_FPS);
  const durationRef = useRef(0);
  const currentFrameRef = useRef(0);
  const observedDeltasRef = useRef<number[]>([]);
  const fpsSampleCountRef = useRef(0);
  const lastMediaTimeRef = useRef<number | null>(null);
  const pendingStepRef = useRef<{ target: number; attempts: number } | null>(null);
  const desiredSeekTimeRef = useRef<number | null>(null);
  const seekInFlightRef = useRef(false);
  const seekRetriesRef = useRef(0);
  const wasPlayingBeforeScrubRef = useRef(false);
  const isScrubbingRef = useRef(false);
  const onMediaErrorRef = useRef(onMediaError);
  useEffect(() => {
    onMediaErrorRef.current = onMediaError;
  });

  const frameSnapshotRef = useRef<VideoFrameState>({ frame: 0, time: 0 });
  const frameListenersRef = useRef(new Set<() => void>());
  const publishFrame = useCallback((frame: number, time: number) => {
    currentFrameRef.current = frame;
    frameSnapshotRef.current = { frame, time };
    frameListenersRef.current.forEach((listener) => listener());
  }, []);
  const subscribeFrames = useCallback((listener: () => void) => {
    frameListenersRef.current.add(listener);
    return () => {
      frameListenersRef.current.delete(listener);
    };
  }, []);
  const getFrame = useCallback(() => frameSnapshotRef.current, []);

  const getTotalFrames = useCallback(() => {
    const timestamps = timestampsRef.current;
    if (timestamps) return timestamps.length;
    const known = durationRef.current;
    return known > 0 ? Math.max(1, Math.round(known * fpsRef.current)) : 0;
  }, []);

  const frameDuration = useCallback((frame: number) => {
    const timestamps = timestampsRef.current;
    if (timestamps && frame >= 0 && frame < timestamps.length - 1) {
      const delta = timestamps[frame + 1] - timestamps[frame];
      if (delta > 0) return delta;
    }
    return 1 / fpsRef.current;
  }, []);

  const midFrameTime = useCallback(
    (frame: number) => {
      const timestamps = timestampsRef.current;
      if (timestamps && frame >= 0 && frame < timestamps.length) {
        return timestamps[frame] + frameDuration(frame) / 2;
      }
      return (frame + 0.5) / fpsRef.current;
    },
    [frameDuration],
  );

  const frameForTime = useCallback(
    (time: number) => {
      const timestamps = timestampsRef.current;
      if (timestamps && timestamps.length > 0) {
        return frameIndexForTime(timestamps, time, frameDuration(0) / 2);
      }
      const total = getTotalFrames();
      return total <= 0 ? 0 : clamp(Math.round(time * fpsRef.current), 0, total - 1);
    },
    [frameDuration, getTotalFrames],
  );

  const performSeek = useCallback(
    (time: number) => {
      const video = videoRef.current;
      if (!video) return;
      // Synthetic frame math can aim past the end, where seeks land short
      // and would retry for nothing.
      const known = durationRef.current;
      const target = known > 0 ? clamp(time, 0, known - 0.001) : Math.max(0, time);
      // A seek to the current position may never fire 'seeked', which would
      // wedge the in-flight flag.
      if (!seekInFlightRef.current && Math.abs(video.currentTime - target) < 1e-6) {
        desiredSeekTimeRef.current = null;
        return;
      }
      desiredSeekTimeRef.current = target;
      seekRetriesRef.current = 0;
      if (!seekInFlightRef.current) {
        seekInFlightRef.current = true;
        video.currentTime = target;
      }
    },
    [videoRef],
  );

  const seekToFrame = useCallback(
    (frame: number) => {
      const video = videoRef.current;
      const total = getTotalFrames();
      if (!video || total <= 0) return;
      const target = clamp(frame, 0, total - 1);
      video.pause();
      pendingStepRef.current = { target, attempts: 0 };
      performSeek(midFrameTime(target));
    },
    [getTotalFrames, midFrameTime, performSeek, videoRef],
  );

  const stepFrame = useCallback(
    (delta: number) => {
      // Rapid steps accumulate from the pending target, not the last frame
      // that happened to paint.
      seekToFrame((pendingStepRef.current?.target ?? currentFrameRef.current) + delta);
    },
    [seekToFrame],
  );

  const play = useCallback(() => {
    videoRef.current?.play().catch(swallowPlayRejection);
  }, [videoRef]);

  const pause = useCallback(() => {
    videoRef.current?.pause();
  }, [videoRef]);

  const togglePlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) play();
    else pause();
  }, [pause, play, videoRef]);

  const scrubToFraction = useCallback(
    (fraction: number) => {
      if (!videoRef.current || durationRef.current <= 0) return;
      pendingStepRef.current = null;
      performSeek(clamp(fraction, 0, 1) * durationRef.current);
    },
    [performSeek, videoRef],
  );

  const beginScrub = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    isScrubbingRef.current = true;
    wasPlayingBeforeScrubRef.current = !video.paused;
    video.pause();
  }, [videoRef]);

  const endScrub = useCallback(() => {
    if (!isScrubbingRef.current) return;
    isScrubbingRef.current = false;
    if (wasPlayingBeforeScrubRef.current) play();
  }, [play]);

  const setLoop = useCallback(
    (value: boolean) => {
      const video = videoRef.current;
      if (video) video.loop = value;
      setLoopState(value);
    },
    [videoRef],
  );

  const setMuted = useCallback(
    (value: boolean) => {
      const video = videoRef.current;
      if (video) video.muted = value;
      setMutedState(value);
      writePrefs({ muted: value, volume: video?.volume ?? 1 });
    },
    [videoRef],
  );

  const setVolume = useCallback(
    (value: number) => {
      const video = videoRef.current;
      const clamped = clamp(value, 0, 1);
      if (video) {
        video.volume = clamped;
        // Dragging the volume up is an unmute.
        if (clamped > 0 && video.muted) video.muted = false;
      }
      setVolumeState(clamped);
      writePrefs({ muted: video ? video.muted : clamped === 0, volume: clamped });
    },
    [videoRef],
  );

  // The real frame index loads in the background; until then (or if the
  // container can't be read) the estimated grid stands.
  useEffect(() => {
    if (!src) return;
    let cancelled = false;
    loadVideoFrameIndex(src)
      .then((index) => {
        if (cancelled || !index) return;
        timestampsRef.current = index.timestamps;
        fpsRef.current = index.fps;
        setFps(index.fps);
        setExactFrameCount(index.timestamps.length);
      })
      .catch((error: unknown) => {
        // Not an error the user can act on: stepping still works on the
        // estimated grid, and the counter shows `~` to say so.
        console.warn("video frame index unavailable; estimating frames", error);
      });
    return () => {
      cancelled = true;
    };
  }, [src]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !src) return;
    const prefs = readPrefs();
    video.muted = prefs?.muted ?? false;
    video.volume = prefs?.volume ?? 1;

    const onLoadedMetadata = () => {
      durationRef.current = video.duration;
      setDuration(video.duration);
      setIsReady(true);
    };
    const onPlay = () => setIsPlaying(true);
    const onPause = () => setIsPlaying(false);
    const onVolumeChange = () => {
      setMutedState(video.muted);
      setVolumeState(video.volume);
    };
    const onSeeked = () => {
      seekInFlightRef.current = false;
      const desired = desiredSeekTimeRef.current;
      // Re-issue a seek that landed off target, but boundedly: a source that
      // only seeks coarsely would otherwise loop identical seeks forever.
      if (
        desired !== null &&
        seekRetriesRef.current < 2 &&
        Math.abs(video.currentTime - desired) > frameDuration(currentFrameRef.current) / 2
      ) {
        seekRetriesRef.current += 1;
        seekInFlightRef.current = true;
        video.currentTime = desired;
        return;
      }
      desiredSeekTimeRef.current = null;
      if (typeof video.requestVideoFrameCallback !== "function") {
        // No rvfc, no landed-frame confirmation. A step aimed mid-frame at its
        // target, so trust the target: currentTime plus the half-frame
        // tolerance meant for frame start times would round to the next one.
        const stepped = pendingStepRef.current?.target;
        pendingStepRef.current = null;
        publishFrame(stepped ?? frameForTime(video.currentTime), video.currentTime);
      }
    };
    const onError = () => onMediaErrorRef.current?.();

    video.addEventListener("loadedmetadata", onLoadedMetadata);
    video.addEventListener("play", onPlay);
    video.addEventListener("pause", onPause);
    video.addEventListener("volumechange", onVolumeChange);
    video.addEventListener("seeked", onSeeked);
    video.addEventListener("error", onError);
    if (video.readyState >= 1) onLoadedMetadata();
    if (autoPlay) video.play().catch(swallowPlayRejection);

    return () => {
      video.removeEventListener("loadedmetadata", onLoadedMetadata);
      video.removeEventListener("play", onPlay);
      video.removeEventListener("pause", onPause);
      video.removeEventListener("volumechange", onVolumeChange);
      video.removeEventListener("seeked", onSeeked);
      video.removeEventListener("error", onError);
    };
    // Once per source: the helpers are stable or read refs.
  }, [src, videoRef]);

  // Displayed-frame tracking.
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !src) return;

    if (typeof video.requestVideoFrameCallback !== "function") {
      const onTimeUpdate = () => {
        const frame = frameForTime(video.currentTime);
        if (frame !== currentFrameRef.current) publishFrame(frame, video.currentTime);
      };
      video.addEventListener("timeupdate", onTimeUpdate);
      return () => video.removeEventListener("timeupdate", onTimeUpdate);
    }

    const refineEstimatedFps = (mediaTime: number, previous: number) => {
      // Seek-driven frames (stepping, scrubbing) have arbitrary deltas that
      // would poison the estimate.
      if (
        isScrubbingRef.current ||
        seekInFlightRef.current ||
        desiredSeekTimeRef.current !== null ||
        pendingStepRef.current !== null
      ) {
        return;
      }
      const delta = mediaTime - previous;
      if (delta <= 0 || delta > 1) return;
      const deltas = observedDeltasRef.current;
      deltas.push(delta);
      if (deltas.length > 60) deltas.shift();
      // The median's sort isn't free at frame rate; sample it periodically.
      fpsSampleCountRef.current += 1;
      if (deltas.length < 10 || fpsSampleCountRef.current % 5 !== 0) return;
      const observed = 1 / median(deltas);
      if (Math.abs(observed - fpsRef.current) / fpsRef.current > 0.05) {
        fpsRef.current = observed;
        setFps(observed);
      }
    };

    const resolvePendingStep = (landed: number) => {
      const pending = pendingStepRef.current;
      if (!pending || seekInFlightRef.current || desiredSeekTimeRef.current !== null) return;
      if (landed === pending.target || pending.attempts >= 2) {
        pendingStepRef.current = null;
        return;
      }
      // Time rounding landed on a neighbor frame: nudge a quarter frame
      // toward the target, at most twice.
      pending.attempts += 1;
      const direction = pending.target > landed ? 1 : -1;
      performSeek(midFrameTime(pending.target) + direction * (frameDuration(pending.target) / 4));
    };

    let cancelled = false;
    let handle = 0;
    const onFrame: VideoFrameRequestCallback = (_now, metadata) => {
      if (cancelled) return;
      const previous = lastMediaTimeRef.current;
      lastMediaTimeRef.current = metadata.mediaTime;
      if (!timestampsRef.current && previous !== null) refineEstimatedFps(metadata.mediaTime, previous);
      const frame = frameForTime(metadata.mediaTime);
      resolvePendingStep(frame);
      if (frame !== currentFrameRef.current) publishFrame(frame, metadata.mediaTime);
      handle = video.requestVideoFrameCallback(onFrame);
    };
    handle = video.requestVideoFrameCallback(onFrame);
    return () => {
      cancelled = true;
      video.cancelVideoFrameCallback(handle);
    };
  }, [src, videoRef]);

  const totalFrames = useMemo(() => {
    if (exactFrameCount !== null) return exactFrameCount;
    return duration > 0 ? Math.max(1, Math.round(duration * fps)) : 0;
  }, [duration, fps, exactFrameCount]);

  // Stable identity during playback: per-frame values flow through the
  // subscription, not this object.
  return useMemo(
    () => ({
      isReady,
      isPlaying,
      duration,
      getFrame,
      subscribeFrames,
      totalFrames,
      frameIndexExact: exactFrameCount !== null,
      loop,
      setLoop,
      muted,
      setMuted,
      volume,
      setVolume,
      play,
      pause,
      togglePlay,
      stepFrame,
      seekToFrame,
      scrubToFraction,
      beginScrub,
      endScrub,
    }),
    [
      isReady,
      isPlaying,
      duration,
      getFrame,
      subscribeFrames,
      totalFrames,
      exactFrameCount,
      loop,
      setLoop,
      muted,
      setMuted,
      volume,
      setVolume,
      play,
      pause,
      togglePlay,
      stepFrame,
      seekToFrame,
      scrubToFraction,
      beginScrub,
      endScrub,
    ],
  );
}
