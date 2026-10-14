// Container-level knowledge the <video> element won't give us: every frame's
// real presentation timestamp (for exact frame stepping and the frame
// counter) and the exact decoded pixels of one frame (for "save frame").
// Both read the file through mediabunny over the asset protocol's range
// requests, so a long video is never pulled into memory whole.

import { ALL_FORMATS, CanvasSink, EncodedPacketSink, Input, UrlSource } from "mediabunny";

export interface VideoFrameIndex {
  /** Presentation timestamps (seconds) of every video frame, ascending. */
  timestamps: number[];
  /** Average frame rate over the timestamp span. */
  fps: number;
}

// The asset protocol is local: a failed read won't succeed on retry.
const openInput = (url: string) =>
  new Input({ source: new UrlSource(url, { getRetryDelay: () => null }), formats: ALL_FORMATS });

/** Every frame's timestamp, demuxed without decoding, or null when the
 *  container can't be parsed (the engine then estimates a frame grid). */
export async function loadVideoFrameIndex(url: string): Promise<VideoFrameIndex | null> {
  const input = openInput(url);
  try {
    const track = await input.getPrimaryVideoTrack();
    if (!track) return null;
    const timestamps: number[] = [];
    const sink = new EncodedPacketSink(track);
    for await (const packet of sink.packets(undefined, undefined, { metadataOnly: true })) {
      // A packet entirely before t=0 is never presented.
      if (packet.timestamp + packet.duration <= 0) continue;
      timestamps.push(packet.timestamp);
    }
    if (timestamps.length === 0) return null;
    // Packets arrive in decode order; B-frames present in a different one.
    timestamps.sort((a, b) => a - b);
    const span = timestamps[timestamps.length - 1] - timestamps[0];
    const fps = timestamps.length > 1 && span > 0 ? (timestamps.length - 1) / span : 30;
    return { timestamps, fps };
  } finally {
    input.dispose();
  }
}

/** The exact frame shown at `time`, decoded at native resolution as PNG, or
 *  null when this webview can't decode the track (the caller then grabs the
 *  <video> element's own pixels). */
export async function extractFramePng(url: string, time: number): Promise<Blob | null> {
  const input = openInput(url);
  try {
    const track = await input.getPrimaryVideoTrack();
    if (!track || !(await track.canDecode())) return null;
    const wrapped = await new CanvasSink(track).getCanvas(Math.max(0, time));
    return wrapped ? await canvasToPng(wrapped.canvas) : null;
  } finally {
    input.dispose();
  }
}

/** The <video> element's current pixels as PNG — the fallback grab. Needs the
 *  element loaded with `crossOrigin` so the canvas isn't tainted. */
export function captureElementFrame(video: HTMLVideoElement): Promise<Blob | null> {
  const canvas = document.createElement("canvas");
  canvas.width = video.videoWidth;
  canvas.height = video.videoHeight;
  const context = canvas.getContext("2d");
  if (!context || canvas.width === 0 || canvas.height === 0) return Promise.resolve(null);
  context.drawImage(video, 0, 0);
  return canvasToPng(canvas);
}

function canvasToPng(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<Blob | null> {
  if (typeof OffscreenCanvas !== "undefined" && canvas instanceof OffscreenCanvas) {
    return canvas.convertToBlob({ type: "image/png" });
  }
  return new Promise((resolve) => (canvas as HTMLCanvasElement).toBlob(resolve, "image/png"));
}
