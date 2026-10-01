// The right-click menu on the file tree: the actions a code editor's explorer
// has, in place of the webview's own (Reload / Inspect Element) menu. Opened
// on a file, a folder, or the empty space below the tree (the project root).
// The panel acts on the pick; this file only decides what is on offer.

import {
  ClipboardCopy,
  Copy,
  CopyPlus,
  Download,
  FileCode2,
  FileDiff,
  FilePlus2,
  FolderOpen,
  FolderPlus,
  Paperclip,
  Pencil,
  RotateCw,
  Trash2,
} from "lucide-react";
import { ContextMenu, MenuItem, MenuSep, type MenuAt } from "../../components/ui/Menu";
import type { FileEntry } from "../../lib/types";

export type FileAction =
  | "open"
  | "diff"
  | "attach"
  | "new-file"
  | "new-folder"
  | "download"
  | "reveal"
  | "copy-path"
  | "copy-relative-path"
  | "rename"
  | "duplicate"
  | "trash"
  | "refresh";

/** What the platform calls its file manager, for "Reveal in …". */
export function fileManagerName(): string {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/Mac/i.test(ua)) return "Finder";
  if (/Win/i.test(ua)) return "File Explorer";
  return "File Manager";
}

export function FileMenu({
  at,
  entry,
  changed = false,
  onClose,
  onPick,
}: {
  at: MenuAt;
  /** The row that was right-clicked; null for the project root. */
  entry: FileEntry | null;
  /** The file has uncommitted changes (offers its diff). */
  changed?: boolean;
  onClose: () => void;
  onPick: (action: FileAction) => void;
}) {
  const item = (action: FileAction, icon: React.ReactNode, name: string, danger = false) => (
    <MenuItem checkSlot={icon} name={name} className={danger ? "danger" : ""} onSelect={() => onPick(action)} />
  );
  const isFile = !!entry && !entry.is_dir;
  const reveal = `Reveal in ${fileManagerName()}`;
  return (
    <ContextMenu at={at} onClose={onClose} label={entry ? `Actions for ${entry.name}` : "Project actions"}>
      {isFile ? (
        <>
          {item("open", <FileCode2 size={14} />, "Open")}
          {changed && item("diff", <FileDiff size={14} />, "View changes")}
          {item("attach", <Paperclip size={14} />, "Add to chat")}
        </>
      ) : (
        <>
          {item("new-file", <FilePlus2 size={14} />, "New file…")}
          {item("new-folder", <FolderPlus size={14} />, "New folder…")}
        </>
      )}
      <MenuSep />
      {isFile && item("download", <Download size={14} />, "Download")}
      {item("reveal", <FolderOpen size={14} />, reveal)}
      <MenuSep />
      {item("copy-path", <Copy size={14} />, "Copy path")}
      {entry && item("copy-relative-path", <ClipboardCopy size={14} />, "Copy relative path")}
      <MenuSep />
      {entry ? (
        <>
          {item("rename", <Pencil size={14} />, "Rename…")}
          {isFile && item("duplicate", <CopyPlus size={14} />, "Duplicate")}
          {item("trash", <Trash2 size={14} />, "Move to Trash", true)}
        </>
      ) : (
        item("refresh", <RotateCw size={14} />, "Refresh")
      )}
    </ContextMenu>
  );
}
