// In-app drag-and-drop of workspace files (tree rows, gallery tiles → chat).
// A custom mime type keeps our drags distinguishable from OS file drops (which
// arrive through Tauri's drag-drop event, not the DOM).

export const FILES_MIME = "application/x-oxen-workspace-files";

/** Stamp a drag with the absolute paths being dragged. */
export function setDragPaths(dt: DataTransfer, absPaths: string[]) {
  dt.setData(FILES_MIME, JSON.stringify(absPaths));
  dt.effectAllowed = "copy";
}

/** Whether a drag carries workspace files (readable during dragover). */
export function hasDragPaths(dt: DataTransfer): boolean {
  return dt.types.includes(FILES_MIME);
}

/** The dragged absolute paths (only readable on drop). */
export function getDragPaths(dt: DataTransfer): string[] {
  try {
    const parsed: unknown = JSON.parse(dt.getData(FILES_MIME) || "[]");
    return Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}
