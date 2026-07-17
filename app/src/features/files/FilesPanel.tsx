// The Files dock: the active workspace as a lazy, collapsible tree. Click a
// file to open it in the Editor pane (⌘-click builds a multi-selection —
// several images open as a gallery grid); drag rows into the chat to attach
// them as context. The header creates files and folders in whichever
// directory is selected, and the tree refreshes itself when the agent
// finishes a turn (it may well have written files).

import { useCallback, useEffect, useRef, useState, type DragEvent, type MouseEvent, type PointerEvent, type ReactNode } from "react";
import {
  ChevronRight,
  FileCode2,
  FilePlus2,
  FileText,
  File as FileIcon,
  Film,
  Folder,
  FolderOpen,
  FolderPlus,
  Image as ImageIcon,
  RotateCw,
} from "lucide-react";
import { fsCreateEntry, fsListDir } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { basename } from "../../lib/format";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import { setDragPaths } from "./dnd";
import type { FileEntry } from "../../lib/types";
import "./files.css";

const CODE_EXTS = new Set([
  "js", "jsx", "ts", "tsx", "rs", "py", "go", "java", "kt", "swift", "c", "h", "cpp", "hpp",
  "rb", "php", "sh", "zsh", "sql", "css", "scss", "html", "json", "jsonl", "toml", "yaml", "yml", "xml",
]);

function iconFor(entry: FileEntry, open: boolean): ReactNode {
  if (entry.is_dir) return open ? <FolderOpen size={14} /> : <Folder size={14} />;
  if (isImagePath(entry.name)) return <ImageIcon size={14} />;
  if (isVideoPath(entry.name)) return <Film size={14} />;
  const ext = entry.name.split(".").pop()?.toLowerCase() ?? "";
  if (CODE_EXTS.has(ext)) return <FileCode2 size={14} />;
  if (ext === "md" || ext === "txt" || ext === "rst") return <FileText size={14} />;
  return <FileIcon size={14} />;
}

const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");

export function FilesPanel({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  const openInViewer = useStore((s) => s.openInViewer);

  /** Loaded directory listings, keyed by workspace-relative dir ("" = root). */
  const [entries, setEntries] = useState<Record<string, FileEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  /** Highlighted rows (⌘-click extends); drives the gallery + multi-drag. */
  const [selected, setSelected] = useState<Set<string>>(new Set());
  /** Where New file / New folder create: the selected row's directory. */
  const [targetDir, setTargetDir] = useState("");
  const [creating, setCreating] = useState<{ dir: string; isDir: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadDir = useCallback(
    async (dir: string) => {
      if (!workspace) return;
      try {
        const list = await fsListDir(workspace, dir);
        setEntries((prev) => ({ ...prev, [dir]: list }));
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [workspace],
  );

  // A different workspace is a different tree: reset and load its root.
  useEffect(() => {
    setEntries({});
    setExpanded(new Set());
    setSelected(new Set());
    setTargetDir("");
    setCreating(null);
    setError(null);
    if (workspace) void loadDir("");
  }, [workspace, loadDir]);

  const refresh = useCallback(() => {
    void loadDir("");
    for (const dir of expanded) void loadDir(dir);
  }, [loadDir, expanded]);

  // The agent writes files during a turn; re-list what's on screen when it ends.
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) refresh();
    wasRunning.current = running;
  }, [running, refresh]);

  // Files changed on disk (any process — the watcher batches them): re-list
  // just the loaded directories that contain a changed path. An empty batch
  // means "too much to enumerate" — refresh everything on screen.
  const fsChange = useStore((s) => s.fsChange);
  useEffect(() => {
    if (!fsChange || !workspace || fsChange.root !== workspace) return;
    if (!fsChange.paths.length) {
      refresh();
      return;
    }
    const dirs = new Set(fsChange.paths.map(parentOf));
    for (const dir of dirs) if (dir === "" || entries[dir]) void loadDir(dir);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fsChange]);

  function toggleDir(path: string) {
    setSelected(new Set([path]));
    setTargetDir(path);
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        if (!entries[path]) void loadDir(path);
      }
      return next;
    });
  }

  function clickFile(e: MouseEvent, path: string) {
    if (e.metaKey || e.ctrlKey) {
      // Build a multi-selection; two or more highlighted images open as a grid.
      const next = new Set(selected);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      setSelected(next);
      const images = [...next].filter(isImagePath);
      if (images.length > 1) openInViewer(images);
      return;
    }
    setSelected(new Set([path]));
    setTargetDir(parentOf(path));
    openInViewer([path]);
  }

  function dragRow(e: DragEvent, entry: FileEntry) {
    if (!workspace || entry.is_dir) return;
    // Dragging a highlighted row carries the whole highlighted group (a
    // multi-selection only ever holds files — see clickFile/toggleDir).
    const group = selected.has(entry.path) && selected.size > 1 ? [...selected] : [entry.path];
    setDragPaths(e.dataTransfer, group.map((p) => `${workspace}/${p}`));
  }

  function startCreate(isDir: boolean) {
    setExpanded((prev) => (targetDir ? new Set(prev).add(targetDir) : prev));
    if (targetDir && !entries[targetDir]) void loadDir(targetDir);
    setCreating({ dir: targetDir, isDir });
  }

  async function submitCreate(name: string) {
    if (!creating || !workspace) return;
    const trimmed = name.trim();
    if (!trimmed) {
      setCreating(null);
      return;
    }
    const rel = creating.dir ? `${creating.dir}/${trimmed}` : trimmed;
    try {
      await fsCreateEntry(workspace, rel, creating.isDir);
      setCreating(null);
      await loadDir(creating.dir);
      if (creating.isDir) {
        setExpanded((prev) => new Set(prev).add(rel));
        setEntries((prev) => ({ ...prev, [rel]: [] }));
        setTargetDir(rel);
      } else {
        setSelected(new Set([rel]));
        openInViewer([rel]);
      }
    } catch (e) {
      setError(String(e));
      setCreating(null);
    }
  }

  function renderNewRow(dir: string, depth: number): ReactNode {
    if (!creating || creating.dir !== dir) return null;
    return (
      <div className="ft-newrow" style={{ paddingLeft: 10 + depth * 14 }}>
        <span className="ft-icon">{creating.isDir ? <FolderPlus size={14} /> : <FilePlus2 size={14} />}</span>
        <input
          autoFocus
          aria-label={creating.isDir ? "New folder name" : "New file name"}
          placeholder={creating.isDir ? "folder name" : "file name"}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submitCreate(e.currentTarget.value);
            if (e.key === "Escape") setCreating(null);
          }}
          onBlur={() => setCreating(null)}
        />
      </div>
    );
  }

  function renderDir(dir: string, depth: number): ReactNode {
    const list = entries[dir];
    if (!list) return null;
    return (
      <>
        {renderNewRow(dir, depth)}
        {list.map((entry) => {
          const open = entry.is_dir && expanded.has(entry.path);
          return (
            <div key={entry.path}>
              <button
                className={`ft-row${selected.has(entry.path) ? " selected" : ""}`}
                style={{ paddingLeft: 10 + depth * 14 }}
                title={entry.path}
                draggable={!entry.is_dir}
                onDragStart={(e) => dragRow(e, entry)}
                onClick={(e) => (entry.is_dir ? toggleDir(entry.path) : clickFile(e, entry.path))}
              >
                {entry.is_dir ? (
                  <ChevronRight size={13} className={`ft-chev${open ? " open" : ""}`} />
                ) : (
                  <span className="ft-chev-slot" />
                )}
                <span className="ft-icon">{iconFor(entry, open)}</span>
                <span className="ft-name">{entry.name}</span>
              </button>
              {open && renderDir(entry.path, depth + 1)}
            </div>
          );
        })}
        {list.length === 0 && dir === "" && <p className="ft-empty">This folder is empty.</p>}
      </>
    );
  }

  if (!workspace) return null;

  return (
    <nav className="files-panel" aria-label="Project files">
      {onResizeStart && (
        <div
          className="files-resizer"
          onPointerDown={onResizeStart}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize files panel"
        />
      )}
      <header className="ft-head">
        <span className="ft-title" title={workspace}>
          {basename(workspace)}
        </span>
        <div className="ft-actions">
          <button
            className="icon-btn sm"
            title="New file"
            aria-label="New file"
            onClick={() => startCreate(false)}
          >
            <FilePlus2 size={14} />
          </button>
          <button
            className="icon-btn sm"
            title="New folder"
            aria-label="New folder"
            onClick={() => startCreate(true)}
          >
            <FolderPlus size={14} />
          </button>
          <button className="icon-btn sm" title="Refresh" aria-label="Refresh files" onClick={refresh}>
            <RotateCw size={13} />
          </button>
        </div>
      </header>
      {error && <p className="ft-error">{error}</p>}
      <div className="ft-tree" role="tree">
        {renderDir("", 0)}
      </div>
      <p className="ft-hint">⌘-click to select a group · drag files into the chat</p>
    </nav>
  );
}
