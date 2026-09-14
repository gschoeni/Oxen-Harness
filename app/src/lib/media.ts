// Helpers shared by the chat's generation cards and the Gallery: reading a
// `generate_image` / `generate_video` call's arguments, and pulling the saved
// files back out of its result text (the result is prose for the model; the
// UI only needs the paths, which are stable `- <path> (…)` lines).

import { isAudioPath, isImagePath, isVideoPath } from "./attachments";
import type { MediaUpload, MediaItem } from "./types";

export const GENERATE_IMAGE_TOOL = "generate_image";
export const GENERATE_VIDEO_TOOL = "generate_video";
export const MEDIA_MODELS_TOOL = "media_models";
export const MEDIA_STATUS_TOOL = "media_status";

/** Mirror of `harness_media::MEDIA_NO_KEY` — the sentence a generation tool
 *  answers with when no Oxen API key is configured. */
export const MEDIA_NO_KEY = "Media generation needs your Oxen API key.";

export const isGenerateTool = (name: string) =>
  name === GENERATE_IMAGE_TOOL || name === GENERATE_VIDEO_TOOL;

/** The parts of a generation call the card shows. */
export interface GenerationArgs {
  kind: "image" | "video";
  prompt: string;
  model?: string;
  refs: string[];
  count: number;
  aspectRatio?: string;
  duration?: number;
  resolution?: string;
  name?: string;
}

export function generationArgs(name: string, a: Record<string, unknown>): GenerationArgs {
  const s = (k: string) => (typeof a[k] === "string" && (a[k] as string).trim() ? (a[k] as string) : undefined);
  const n = (k: string) => (typeof a[k] === "number" ? (a[k] as number) : undefined);
  const refs = Array.isArray(a.refs) ? (a.refs as unknown[]).filter((r): r is string => typeof r === "string") : [];
  return {
    kind: name === GENERATE_VIDEO_TOOL ? "video" : "image",
    prompt: s("prompt") ?? "",
    model: s("model"),
    refs,
    count: Math.min(4, Math.max(1, Math.round(n("count") ?? 1))),
    aspectRatio: s("aspect_ratio"),
    duration: n("duration"),
    resolution: s("resolution"),
    name: s("name"),
  };
}

/** One saved output parsed from a result line. */
export interface GeneratedFile {
  path: string;
  poster?: string;
  kind: "image" | "video";
  /** `1024×576` when the line carried it. */
  size?: string;
}

const MEDIA_EXT = /\.(png|jpe?g|webp|gif|mp4|webm|mov)$/i;

/** The files a generation result names: lines like
 *  `- generations/2026-09-13/1402-slug-1.png (1024×576)` or
 *  `- generations/…/x.mp4 (8.1 MB), 8s — poster generations/…/x.jpg`. */
export function generatedFiles(result: string): GeneratedFile[] {
  const out: GeneratedFile[] = [];
  for (const raw of result.split("\n")) {
    const line = raw.trim();
    if (!line.startsWith("- ")) continue;
    const body = line.slice(2);
    const path = body.split(/\s/)[0];
    if (!path || !MEDIA_EXT.test(path)) continue;
    const kind: GeneratedFile["kind"] = isVideoPath(path) ? "video" : "image";
    const size = body.match(/\((\d+×\d+)\)/)?.[1];
    const poster = body.match(/poster\s+(\S+\.(?:png|jpe?g|webp))/i)?.[1];
    out.push({ path, kind, size, poster });
  }
  return out;
}

/** `image` / `video` / `audio` for a path, or null. */
export function mediaKindOf(path: string): "image" | "video" | "audio" | null {
  if (isImagePath(path)) return "image";
  if (isVideoPath(path)) return "video";
  if (isAudioPath(path)) return "audio";
  return null;
}

/** Aspect ratio as a CSS `aspect-ratio` value: `"16:9"` → `"16 / 9"`; an
 *  item's real dimensions when known; square otherwise. */
export function aspectFor(item: { width?: number | null; height?: number | null } | null, ratio?: string): string {
  if (item?.width && item?.height) return `${item.width} / ${item.height}`;
  const m = ratio?.match(/^(\d+):(\d+)$/);
  if (m) return `${m[1]} / ${m[2]}`;
  return "1 / 1";
}

export const isInFlight = (item: MediaItem) => item.status === "queued" || item.status === "processing";

/** `$0.01`, `$1.38`; four decimals under a cent so a draft isn't "free". */
export function fmtUsd(usd: number): string {
  return usd > 0 && usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`;
}

/** `today 14:02`, `Sep 10`. */
export function whenLabel(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const now = new Date();
  const hhmm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  if (d.toDateString() === now.toDateString()) return `today ${hhmm}`;
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${hhmm}`;
}

/** `8s`, `1m 24s` for an in-flight item's age. */
export function elapsedSince(unixSecs: number, nowMs: number): string {
  const s = Math.max(0, Math.floor(nowMs / 1000) - unixSecs);
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${s % 60}s`;
}

/** A project's items from the store's root-keyed slice. The backend emits the
 *  canonical root; a project path may differ by a trailing slash. */
export function mediaForRoot(
  media: Record<string, MediaItem[] | undefined>,
  root: string,
): MediaItem[] | undefined {
  const norm = root.replace(/\/+$/, "") || root;
  return media[norm] ?? media[root] ?? media[`${norm}/`];
}

/** A project path as the backend keys its library: no trailing slash. */
export const mediaRoot = (path: string) => path.replace(/\/+$/, "") || path;

/** `23 generations · 4 videos`, `1 generation`, `no generations`. */
export function mediaCountLabel(items: MediaItem[]): string {
  const done = items.filter((i) => i.status === "succeeded");
  if (done.length === 0) return "no generations";
  const videos = done.filter((i) => i.kind === "video").length;
  const base = `${done.length} generation${done.length === 1 ? "" : "s"}`;
  return videos > 0 ? `${base} · ${videos} video${videos === 1 ? "" : "s"}` : base;
}

/** An upload still worth showing: in flight, or failed (so the error is seen). */
export const isActiveUpload = (u: MediaUpload) => u.status !== "done" && u.status !== "reused";

/** `1.2 MB`, `640 KB`. */
export function fmtBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

/** 0–100, clamped; 100 once the bytes are all sent. */
export function uploadPercent(u: MediaUpload): number {
  if (u.status === "done" || u.status === "reused") return 100;
  if (u.bytes_total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((u.bytes_sent / u.bytes_total) * 100)));
}
