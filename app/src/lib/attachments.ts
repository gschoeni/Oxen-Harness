// Shared helpers for attachment display.

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "ico"]);

const VIDEO_EXTS = new Set(["mp4", "webm", "mov", "m4v", "ogv"]);

/** Whether a path/filename looks like an image we can preview. */
export function isImagePath(path: string): boolean {
  const ext = path.split(/[./\\]/).pop()?.toLowerCase();
  return !!ext && IMAGE_EXTS.has(ext);
}

/** Whether a path/filename looks like a video the viewer can play. */
export function isVideoPath(path: string): boolean {
  const ext = path.split(/[./\\]/).pop()?.toLowerCase();
  return !!ext && VIDEO_EXTS.has(ext);
}
