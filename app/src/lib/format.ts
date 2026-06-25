/** A short relative timestamp like "just now", "12m ago", "3d ago", or a date. */
export function relativeTime(secs: number): string {
  const then = secs * 1000;
  const mins = Math.floor((Date.now() - then) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.floor(hrs / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(then).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** Human-readable byte size (e.g. "3.4 GB"). */
export function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return u === 0 ? `${bytes} B` : `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}

/** Running duration like `120ms`, `7s`, or `1m07s` (mirrors the CLI spinner).
 *  Sub-second durations report milliseconds so fast tool calls aren't all `0s`. */
export function elapsed(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  const secs = Math.floor(ms / 1000);
  if (secs < 60) return `${secs}s`;
  return `${Math.floor(secs / 60)}m${String(secs % 60).padStart(2, "0")}s`;
}

/** Clamp a string to `max` characters, appending an ellipsis when truncated. */
export function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max) + "…" : s;
}

/** The final path segment of a file path (handles both `/` and `\`). */
export function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}
