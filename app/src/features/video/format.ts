// Pure readouts for the video controls, kept apart so they test in plain node.

/** `m:ss.mmm` — millisecond precision, since frame stepping moves ~33ms. */
export function formatTimecode(time: number): string {
  const safe = Math.max(0, time);
  const minutes = Math.floor(safe / 60);
  const seconds = Math.floor(safe % 60);
  const millis = Math.floor((safe % 1) * 1000);
  return `${minutes}:${String(seconds).padStart(2, "0")}.${String(millis).padStart(3, "0")}`;
}

/** 1-based `current/total`, zero-padded to a fixed width so the readout
 *  doesn't jitter. A leading `~` marks an estimated frame grid (the container
 *  couldn't be indexed). */
export function formatFrameCounter(frame: number, total: number, exact: boolean): string {
  const width = String(total).length;
  const current = String(frame + 1).padStart(width, "0");
  return `${exact ? "" : "~"}${current}/${total}`;
}

/** `clip_frame_12.png` for frame index 11 of `dir/clip.mp4`. */
export function frameFilename(path: string, frame: number): string {
  const name = path.split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  return `${stem || "video"}_frame_${frame + 1}.png`;
}
