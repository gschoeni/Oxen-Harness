// The Files dock: the active workspace as a lazy, collapsible tree. Click a
// file to open it in the Editor pane (⌘-click builds a multi-selection —
// several images open as a gallery grid); drag rows into the chat to attach
// them as context. The header creates files and folders in whichever
// directory is selected, and the tree refreshes itself when the agent
// finishes a turn (it may well have written files). Right-click a row (or the
// empty space below the tree) for the explorer's actions: see FileMenu.tsx.
// Files dropped from another app (Finder, a browser) are copied into the
// folder under the pointer — or the folder holding the file under it.

import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type MouseEvent, type PointerEvent, type ReactNode } from "react";
import {
  ChevronRight,
  Download,
  FileCode2,
  FileDiff,
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
import {
  fsCreateEntry,
  fsDownload,
  fsDuplicate,
  fsImportFile,
  fsListDir,
  fsRename,
  fsReveal,
  fsTrash,
} from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { basename } from "../../lib/format";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import { hasDragPaths, readBytes, setDragPaths } from "./dnd";
import { diffTab, statusLetter } from "./diff";
import { useGitStatus } from "./useGitStatus";
import { FileMenu, type FileAction } from "./FileMenu";
import type { MenuAt } from "../../components/ui/Menu";
import type { FileEntry, GitFileState } from "../../lib/types";
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

/** Whether `dir` is under `ancestor` ("" is the root and contains everything). */
const isUnder = (dir: string, ancestor: string) => ancestor === "" || dir.startsWith(`${ancestor}/`);

/** Whether every ancestor of `dir` is expanded — i.e. its rows are on screen. */
function isVisible(dir: string, expanded: Set<string>): boolean {
  for (let p = parentOf(dir); p !== ""; p = parentOf(p)) if (!expanded.has(p)) return false;
  return true;
}

/** `path` after `from` (itself or an ancestor of it) was renamed to `to`. */
const rebase = (path: string, from: string, to: string) =>
  path === from || isUnder(path, from) ? to + path.slice(from.length) : path;

/** How long a drag must rest on a closed folder before it opens, so a file
 *  can be dropped deep in the tree without letting go first. */
const DRAG_EXPAND_MS = 600;

/** Whether a drag carries files from another app (not our own tree rows). */
const isOsFileDrag = (dt: DataTransfer) => dt.types.includes("Files") && !hasDragPaths(dt);

/** The folder a drop at `target` lands in: a folder row itself, a file row's
 *  folder, or the root for the empty space below the rows. */
function dropDirAt(target: EventTarget | null): string {
  const row = target instanceof Element ? target.closest<HTMLElement>("[data-path]") : null;
  if (!row) return "";
  const path = row.dataset.path ?? "";
  return row.dataset.dir === "true" ? path : parentOf(path);
}

/** The dropped files, minus folders (which arrive as unreadable `File`s): a
 *  folder's contents are not copied, and it is named so the user knows. */
function droppedFiles(dt: DataTransfer): { files: File[]; folders: string[] } {
  const files: File[] = [];
  const folders: string[] = [];
  const items = Array.from(dt.items ?? []);
  Array.from(dt.files).forEach((file, i) => {
    const entry = items[i]?.webkitGetAsEntry?.();
    if (entry?.isDirectory) folders.push(file.name);
    else files.push(file);
  });
  return { files, folders };
}

const shownDir = (dir: string) => (dir ? `${dir}/` : "the project root");

/** The problem with `name` as a new entry in a folder holding `taken`, or
 *  null when it is fine. Case-insensitive: macOS volumes usually are. */
function nameProblem(name: string, taken: string[]): string | null {
  const trimmed = name.trim();
  if (!trimmed) return null;
  if (trimmed.includes("/")) return "A name can't contain “/”.";
  if (trimmed === "." || trimmed === "..") return "That name is reserved.";
  const lower = trimmed.toLowerCase();
  if (taken.some((t) => t.toLowerCase() === lower)) return `“${trimmed}” already exists here.`;
  return null;
}

/** The inline name field for creating or renaming an entry: sits in the row
 *  it will become, says where it lands, and flags a clash before Enter. Enter
 *  (or clicking away) commits a valid name; Escape or an empty one cancels. */
function NameField({
  depth,
  icon,
  label,
  placeholder,
  defaultValue = "",
  where,
  taken,
  selectStem = false,
  onSubmit,
  onCancel,
}: {
  depth: number;
  icon: ReactNode;
  label: string;
  placeholder?: string;
  defaultValue?: string;
  where: string;
  taken: string[];
  /** Select the name up to its extension: the part being retyped. */
  selectStem?: boolean;
  onSubmit: (name: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(defaultValue);
  // Enter unmounts the field, and a blur can still follow; settle once.
  const settled = useRef(false);
  const problem = nameProblem(value, taken);
  function settle(commit: boolean) {
    if (settled.current) return;
    settled.current = true;
    if (commit && value.trim() && !problem) onSubmit(value);
    else onCancel();
  }
  return (
    // The field keeps the text-editing menu (paste a name), not the tree's.
    <div
      className={`ft-newrow${problem ? " invalid" : ""}`}
      style={{ paddingLeft: 10 + depth * 14 }}
      onContextMenu={(e) => e.stopPropagation()}
    >
      <div className="ft-newrow-field">
        <span className="ft-chev-slot" />
        <span className="ft-icon">{icon}</span>
        <input
          autoFocus
          aria-label={label}
          aria-invalid={!!problem}
          placeholder={placeholder}
          value={value}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setValue(e.currentTarget.value)}
          onFocus={(e) => {
            const dot = selectStem ? defaultValue.lastIndexOf(".") : -1;
            e.currentTarget.setSelectionRange(0, dot > 0 ? dot : defaultValue.length);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !problem) settle(true);
            if (e.key === "Escape") settle(false);
          }}
          onBlur={() => settle(true)}
        />
      </div>
      <p className="ft-newrow-hint" role={problem ? "alert" : undefined}>
        {problem ?? (
          <>
            in {where}
            <span className="ft-keys">
              <kbd>↵</kbd> save <kbd>esc</kbd> cancel
            </span>
          </>
        )}
      </p>
    </div>
  );
}

/** How long a "Saved to …" toast stays up at the foot of the panel. */
const NOTE_MS = 4000;

/** The most directory listings kept in memory at once. Listings are only
 *  cached for directories whose rows are on screen (a collapse releases its
 *  subtree), so this is a backstop for a tree with hundreds of folders open at
 *  once, not the working bound. */
const MAX_CACHED_DIRS = 200;

/** `entries` with `dir` (re)inserted last — insertion order doubles as the LRU
 *  order — and, past the cap, the oldest listings that are not on screen
 *  evicted. On-screen listings are never evicted: they are what the tree is
 *  rendering. */
function cacheListing(
  prev: Record<string, FileEntry[]>,
  dir: string,
  list: FileEntry[],
  expanded: Set<string>,
): Record<string, FileEntry[]> {
  const next: Record<string, FileEntry[]> = {};
  for (const [d, l] of Object.entries(prev)) if (d !== dir) next[d] = l;
  next[dir] = list;
  const keys = Object.keys(next);
  if (keys.length > MAX_CACHED_DIRS) {
    let excess = keys.length - MAX_CACHED_DIRS;
    for (const d of keys) {
      if (excess === 0) break;
      if (d === "" || d === dir || (expanded.has(d) && isVisible(d, expanded))) continue;
      delete next[d];
      excess--;
    }
  }
  return next;
}

/** `entries` without `dir`'s listing and every listing below it. */
function dropSubtree(prev: Record<string, FileEntry[]>, dir: string): Record<string, FileEntry[]> {
  const next: Record<string, FileEntry[]> = {};
  for (const [d, l] of Object.entries(prev)) if (d !== dir && !isUnder(d, dir)) next[d] = l;
  return next;
}

export function FilesPanel({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  const openInViewer = useStore((s) => s.openInViewer);
  const stageAttachment = useStore((s) => s.stageAttachment);
  const retargetEditorPaths = useStore((s) => s.retargetEditorPaths);
  const filesReveal = useStore((s) => s.filesReveal);
  const clearFilesReveal = useStore((s) => s.clearFilesReveal);
  /** A reveal in progress: the path whose row to scroll to once its
   *  ancestors have listed. */
  const revealing = useRef<string | null>(null);

  /** Directory listings for what the tree is showing, keyed by
   *  workspace-relative dir ("" = root). Not a history of everything ever
   *  opened: collapsing a folder releases its subtree's listings (a project
   *  with node_modules browsed once would otherwise pin thousands of entries
   *  and re-list them on every watcher batch), and `cacheListing` bounds the
   *  rest. Re-expanding re-lists — cheap, and it is fresher for it. */
  const [entries, setEntries] = useState<Record<string, FileEntry[]>>({});
  /** Folders the user has opened. Kept across a parent's collapse so that
   *  re-opening the parent shows the same shape it had; their listings are
   *  re-fetched then (see `expandDir`). */
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  // The latest expanded set for callbacks that must not re-create per change.
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  /** Highlighted rows (⌘-click extends); drives the gallery + multi-drag. */
  const [selected, setSelected] = useState<Set<string>>(new Set());
  /** Where New file / New folder create: the selected row's directory. */
  const [targetDir, setTargetDir] = useState("");
  const [creating, setCreating] = useState<{ dir: string; isDir: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The open right-click menu: where, and on which row (null = the root). */
  const [menu, setMenu] = useState<{ at: MenuAt; entry: FileEntry | null } | null>(null);
  /** The row being renamed in place. */
  const [renaming, setRenaming] = useState<string | null>(null);
  /** A passing confirmation (where a download landed). */
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    if (!note) return;
    const timer = window.setTimeout(() => setNote(null), NOTE_MS);
    return () => window.clearTimeout(timer);
  }, [note]);
  const [changesOpen, setChangesOpen] = useState(true);
  /** The folder an OS file drag is over ("" = the root), null when none is. */
  const [dropDir, setDropDir] = useState<string | null>(null);
  // dragenter/dragleave fire for every row crossed; only the outermost leave
  // ends the hover.
  const dragDepth = useRef(0);
  const expandTimer = useRef<{ dir: string; id: number } | null>(null);

  /** Changed files per git; null = not a repository (section hidden). The
   *  hook owns every refresh trigger except the manual Refresh button. */
  const git = useGitStatus(workspace);
  const refreshGit = useStore((s) => s.refreshGitStatus);
  const loadGit = useCallback(() => {
    if (workspace) void refreshGit(workspace);
  }, [workspace, refreshGit]);

  const loadDir = useCallback(
    async (dir: string) => {
      if (!workspace) return;
      try {
        const list = await fsListDir(workspace, dir);
        setEntries((prev) => cacheListing(prev, dir, list, expandedRef.current));
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
    setMenu(null);
    setRenaming(null);
    setNote(null);
    setError(null);
    setDropDir(null);
    if (workspace) void loadDir("");
  }, [workspace, loadDir]);

  // Only the listings on screen are re-fetched: an expanded folder under a
  // collapsed parent has no rows showing (and no cached listing to refresh).
  const refresh = useCallback(() => {
    void loadDir("");
    for (const dir of expanded) if (isVisible(dir, expanded)) void loadDir(dir);
    void loadGit();
  }, [loadDir, loadGit, expanded]);

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

  // "Reveal in Files": open every ancestor of the path, select its row, and
  // scroll to it once the listings have arrived (below, on `entries`).
  useEffect(() => {
    if (!filesReveal || !workspace) return;
    const path = filesReveal.path;
    const ancestors: string[] = [];
    for (let p = parentOf(path); p !== ""; p = parentOf(p)) ancestors.unshift(p);
    setExpanded((prev) => {
      const next = new Set(prev);
      for (const dir of ancestors) next.add(dir);
      return next;
    });
    for (const dir of ancestors) void loadDir(dir);
    setSelected(new Set([path]));
    setTargetDir(parentOf(path));
    revealing.current = path;
    clearFilesReveal();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filesReveal]);
  useEffect(() => {
    const path = revealing.current;
    if (!path) return;
    const row = document.querySelector<HTMLElement>(`.ft-row[data-path="${CSS.escape(path)}"]`);
    if (!row) return;
    row.scrollIntoView?.({ block: "center" });
    revealing.current = null;
  }, [entries]);

  /** Git state per changed path, for the tree rows' badges. */
  const gitByPath = useMemo(() => {
    const map = new Map<string, GitFileState>();
    for (const state of git ?? []) map.set(state.path, state);
    return map;
  }, [git]);

  /** Directories with a change somewhere below, for the folder dot. */
  const dirsChanged = useMemo(() => {
    const dirs = new Set<string>();
    for (const state of git ?? []) {
      let dir = parentOf(state.path);
      while (dir) {
        dirs.add(dir);
        dir = parentOf(dir);
      }
    }
    return dirs;
  }, [git]);

  const openDiff = useCallback(
    (path: string) => openInViewer([diffTab(path)]),
    [openInViewer],
  );

  /** Open a folder: list it, plus any folders below it that are still marked
   *  expanded — their listings were released when this one collapsed, and
   *  they need to be back before their rows can show. */
  function expandDir(path: string, prev: Set<string>): Set<string> {
    const next = new Set(prev).add(path);
    if (!entries[path]) void loadDir(path);
    for (const dir of next) {
      if (dir !== path && isUnder(dir, path) && !entries[dir] && isVisible(dir, next)) void loadDir(dir);
    }
    return next;
  }

  function toggleDir(path: string) {
    setSelected(new Set([path]));
    setTargetDir(path);
    if (expanded.has(path)) {
      // Collapse: the rows disappear, so their listings go too. The nested
      // expanded flags stay, so re-opening restores the same shape.
      setEntries((prev) => dropSubtree(prev, path));
      setExpanded((prev) => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
    } else {
      setExpanded((prev) => expandDir(path, prev));
    }
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

  function startCreate(isDir: boolean, dir = targetDir) {
    if (dir && !expanded.has(dir)) setExpanded((prev) => expandDir(dir, prev));
    setCreating({ dir, isDir });
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

  function openMenu(e: MouseEvent, entry: FileEntry | null) {
    e.preventDefault();
    e.stopPropagation();
    if (entry) {
      // The menu acts on the row under the pointer, so that row is the selection.
      setSelected(new Set([entry.path]));
      setTargetDir(entry.is_dir ? entry.path : parentOf(entry.path));
    }
    setMenu({ at: { x: e.clientX, y: e.clientY }, entry });
  }

  async function pickAction(action: FileAction) {
    if (!menu || !workspace) return;
    const path = menu.entry?.path ?? "";
    setMenu(null);
    try {
      switch (action) {
        case "open":
          openInViewer([path]);
          break;
        case "diff":
          openDiff(path);
          break;
        case "attach":
          stageAttachment(`${workspace}/${path}`);
          break;
        case "new-file":
          startCreate(false, path);
          break;
        case "new-folder":
          startCreate(true, path);
          break;
        case "download":
          setNote(`Saved to ${await fsDownload(workspace, path)}`);
          break;
        case "reveal":
          await fsReveal(workspace, path);
          break;
        case "copy-path":
          await navigator.clipboard.writeText(path ? `${workspace}/${path}` : workspace);
          break;
        case "copy-relative-path":
          await navigator.clipboard.writeText(path);
          break;
        case "rename":
          setRenaming(path);
          break;
        case "duplicate": {
          const copy = await fsDuplicate(workspace, path);
          await loadDir(parentOf(path));
          setSelected(new Set([copy]));
          break;
        }
        case "trash":
          await fsTrash(workspace, path);
          setEntries((prev) => dropSubtree(prev, path));
          setExpanded((prev) => new Set([...prev].filter((d) => d !== path && !isUnder(d, path))));
          setSelected(new Set());
          if (targetDir === path || isUnder(targetDir, path)) setTargetDir(parentOf(path));
          retargetEditorPaths(path, null);
          await loadDir(parentOf(path));
          loadGit();
          break;
        case "refresh":
          refresh();
          break;
      }
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function submitRename(name: string) {
    const from = renaming;
    setRenaming(null);
    if (!from || !workspace) return;
    const trimmed = name.trim();
    if (!trimmed || trimmed === basename(from)) return;
    try {
      const to = await fsRename(workspace, from, trimmed);
      // A renamed folder keeps its open shape: the expanded flags move with
      // it, and the listings (keyed by the old paths) are fetched again.
      const next = new Set([...expandedRef.current].map((d) => rebase(d, from, to)));
      setExpanded(next);
      setEntries((prev) => dropSubtree(prev, from));
      setSelected(new Set([to]));
      setTargetDir((dir) => rebase(dir, from, to));
      retargetEditorPaths(from, to);
      await loadDir(parentOf(from));
      for (const dir of next) if ((dir === to || isUnder(dir, to)) && isVisible(dir, next)) void loadDir(dir);
      loadGit();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  /** The names already in `dir`, for the name field's clash check. */
  const namesIn = (dir: string, except?: string) =>
    (entries[dir] ?? []).filter((e) => e.path !== except).map((e) => e.name);

  function renderRenameRow(entry: FileEntry, depth: number, open: boolean): ReactNode {
    const dir = parentOf(entry.path);
    return (
      <NameField
        depth={depth}
        icon={iconFor(entry, open)}
        label={`Rename ${entry.name}`}
        defaultValue={entry.name}
        selectStem={!entry.is_dir}
        where={shownDir(dir)}
        taken={namesIn(dir, entry.path)}
        onSubmit={(name) => void submitRename(name)}
        onCancel={() => setRenaming(null)}
      />
    );
  }

  function renderNewRow(dir: string, depth: number): ReactNode {
    if (!creating || creating.dir !== dir) return null;
    return (
      <NameField
        depth={depth}
        icon={creating.isDir ? <Folder size={14} /> : <FileIcon size={14} />}
        label={creating.isDir ? "New folder name" : "New file name"}
        placeholder={creating.isDir ? "New folder" : "new-file.md"}
        where={shownDir(dir)}
        taken={namesIn(dir)}
        onSubmit={(name) => void submitCreate(name)}
        onCancel={() => setCreating(null)}
      />
    );
  }

  // ---- files dropped from other apps ---------------------------------------

  function clearExpandTimer() {
    if (expandTimer.current) window.clearTimeout(expandTimer.current.id);
    expandTimer.current = null;
  }
  useEffect(() => clearExpandTimer, []);

  function endOsDrag() {
    dragDepth.current = 0;
    clearExpandTimer();
    setDropDir(null);
  }

  function onTreeDragEnter(e: DragEvent) {
    if (!isOsFileDrag(e.dataTransfer)) return;
    dragDepth.current += 1;
  }

  function onTreeDragLeave(e: DragEvent) {
    if (!isOsFileDrag(e.dataTransfer)) return;
    dragDepth.current = Math.max(0, dragDepth.current - 1);
    if (dragDepth.current === 0) endOsDrag();
  }

  function onTreeDragOver(e: DragEvent) {
    if (!isOsFileDrag(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = "copy";
    const dir = dropDirAt(e.target);
    if (dir !== dropDir) setDropDir(dir);
    // Resting on a closed folder opens it, so the drop can go deeper.
    if (expandTimer.current?.dir === dir) return;
    clearExpandTimer();
    if (dir && !expanded.has(dir)) {
      const id = window.setTimeout(() => {
        expandTimer.current = null;
        setExpanded((prev) => (prev.has(dir) ? prev : expandDir(dir, prev)));
      }, DRAG_EXPAND_MS);
      expandTimer.current = { dir, id };
    }
  }

  async function onTreeDrop(e: DragEvent) {
    if (!isOsFileDrag(e.dataTransfer) || !workspace) return;
    e.preventDefault();
    e.stopPropagation();
    const dir = dropDirAt(e.target);
    endOsDrag();
    const { files, folders } = droppedFiles(e.dataTransfer);
    const copied: string[] = [];
    const problems: string[] = [];
    for (const file of files) {
      try {
        const bytes = new Uint8Array(await readBytes(file));
        copied.push(await fsImportFile(workspace, dir, file.name, bytes));
      } catch (err) {
        problems.push(`Couldn't copy ${file.name}: ${String(err)}`);
      }
    }
    if (folders.length) problems.push(`Folders aren't copied yet (${folders.join(", ")}): drop the files inside.`);
    if (copied.length) {
      if (dir) setExpanded((prev) => (prev.has(dir) ? prev : expandDir(dir, prev)));
      await loadDir(dir);
      setSelected(new Set(copied));
      setTargetDir(dir);
      setNote(`Copied ${copied.length === 1 ? basename(copied[0]) : `${copied.length} files`} into ${shownDir(dir)}`);
      loadGit();
    }
    // After the re-list, which clears the error line when it succeeds.
    setError(problems.length ? problems.join(" · ") : null);
  }

  function renderDir(dir: string, depth: number): ReactNode {
    const list = entries[dir];
    if (!list) return null;
    return (
      <>
        {renderNewRow(dir, depth)}
        {list.map((entry) => {
          const open = entry.is_dir && expanded.has(entry.path);
          const state = entry.is_dir ? undefined : gitByPath.get(entry.path);
          return (
            <div key={entry.path} className={entry.is_dir && dropDir === entry.path ? "ft-dropzone" : undefined}>
              {renaming === entry.path ? (
                renderRenameRow(entry, depth, open)
              ) : (
              <button
                className={`ft-row${selected.has(entry.path) ? " selected" : ""}${state ? ` git-${state.status}` : ""}`}
                data-path={entry.path}
                data-dir={entry.is_dir || undefined}
                style={{ paddingLeft: 10 + depth * 14 }}
                title={state ? `${entry.path} — ${state.status}` : entry.path}
                draggable={!entry.is_dir}
                onDragStart={(e) => dragRow(e, entry)}
                onClick={(e) => (entry.is_dir ? toggleDir(entry.path) : clickFile(e, entry.path))}
                onContextMenu={(e) => openMenu(e, entry)}
              >
                {entry.is_dir ? (
                  <ChevronRight size={13} className={`ft-chev${open ? " open" : ""}`} />
                ) : (
                  <span className="ft-chev-slot" />
                )}
                <span className="ft-icon">{iconFor(entry, open)}</span>
                <span className="ft-name">{entry.name}</span>
                {entry.is_dir && dirsChanged.has(entry.path) && (
                  <span className="ft-dirdot" title="Contains changes" aria-hidden="true" />
                )}
                {state && (
                  <span className="ft-badge" aria-label={state.status}>
                    {statusLetter[state.status]}
                  </span>
                )}
              </button>
              )}
              {open && renderDir(entry.path, depth + 1)}
            </div>
          );
        })}
        {list.length === 0 && dir === "" && <p className="ft-empty">This folder is empty.</p>}
      </>
    );
  }

  // No project yet: keep the column's chrome and say where a project comes
  // from.
  if (!workspace) {
    return (
      <nav className="files-panel" aria-label="Project files">
        <header className="ft-head">
          <span className="ft-title">No project</span>
        </header>
        <p className="ft-empty">Pick a project from Home to browse its files here.</p>
      </nav>
    );
  }

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
            <FilePlus2 size={15} />
          </button>
          <button
            className="icon-btn sm"
            title="New folder"
            aria-label="New folder"
            onClick={() => startCreate(true)}
          >
            <FolderPlus size={15} />
          </button>
          <button className="icon-btn sm" title="Refresh" aria-label="Refresh files" onClick={refresh}>
            <RotateCw size={15} />
          </button>
        </div>
      </header>
      {error && <p className="ft-error">{error}</p>}
      {git !== null && git.length > 0 && (
        <section className="ft-changes" aria-label="Git changes">
          <button
            className="ft-changes-head"
            aria-expanded={changesOpen}
            onClick={() => setChangesOpen((open) => !open)}
          >
            <ChevronRight size={13} className={`ft-chev${changesOpen ? " open" : ""}`} />
            <span className="ft-changes-title">Changes</span>
            <span className="ft-changes-count">{git.length}</span>
          </button>
          {changesOpen && (
            <div className="ft-changes-list">
              {git.map((state) => {
                const dir = parentOf(state.path);
                const renamed = state.original_path ? `${state.original_path} → ${state.path}` : state.path;
                return (
                  <div key={state.path} className={`ft-change git-${state.status}`}>
                    <button
                      className="ft-change-main"
                      title={`${renamed} — ${state.status} · click to view the diff`}
                      onClick={() => openDiff(state.path)}
                    >
                      <FileDiff size={13} aria-hidden="true" />
                      <span className="ft-change-name">{basename(state.path)}</span>
                      {dir && <span className="ft-change-dir">{dir}</span>}
                    </button>
                    {state.status !== "deleted" && (
                      <button
                        className="ft-change-open"
                        title={`Open ${basename(state.path)}`}
                        aria-label={`Open ${basename(state.path)}`}
                        onClick={() => openInViewer([state.path])}
                      >
                        <FileCode2 size={12} />
                      </button>
                    )}
                    <span className="ft-badge" title={state.status} aria-label={state.status}>
                      {statusLetter[state.status]}
                    </span>
                  </div>
                );
              })}
            </div>
          )}
        </section>
      )}
      <div
        className={`ft-tree${dropDir === "" ? " ft-dropzone-root" : ""}`}
        role="tree"
        onContextMenu={(e) => openMenu(e, null)}
        onDragEnter={onTreeDragEnter}
        onDragLeave={onTreeDragLeave}
        onDragOver={onTreeDragOver}
        onDrop={(e) => void onTreeDrop(e)}
      >
        {renderDir("", 0)}
      </div>
      {dropDir !== null && (
        <p className="ft-drop-hint" aria-live="polite">
          <Download size={13} aria-hidden="true" />
          Copy into <strong>{shownDir(dropDir)}</strong>
        </p>
      )}
      {menu && (
        <FileMenu
          at={menu.at}
          entry={menu.entry}
          changed={!!menu.entry && gitByPath.has(menu.entry.path)}
          onClose={() => setMenu(null)}
          onPick={(action) => void pickAction(action)}
        />
      )}
      <p className="ft-hint">⌘-click to select a group · drag files into the chat · drop files here to copy them in</p>
      {note && (
        <p className="ft-note" role="status">
          {note}
        </p>
      )}
    </nav>
  );
}
