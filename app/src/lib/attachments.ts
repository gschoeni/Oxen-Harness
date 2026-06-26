// Shared helpers for attachment display.

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "ico"]);

/** Whether a path/filename looks like an image we can preview. */
export function isImagePath(path: string): boolean {
  const ext = path.split(/[./\\]/).pop()?.toLowerCase();
  return !!ext && IMAGE_EXTS.has(ext);
}
