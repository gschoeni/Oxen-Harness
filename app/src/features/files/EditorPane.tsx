// Shared drafts survive navigation; the file picker selects a resource within
// the conversation context. Media can be dragged into chat as attachments.

import {
  useCallback,
  useMemo,
  useEffect,
  useState,
  type DragEvent,
  type PointerEvent,
} from "react";
import {
  Check,
  Code2,
  Eye,
  FileCode2,
  FileDiff,
  Film,
  Image as ImageIcon,
  MessageSquarePlus,
  Save,
  WrapText,
  X,
} from "lucide-react";
import { fsReadFile } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { useDocument } from "../../workbench-sdk";
import { useWorkbenchAPI } from "../workbench/api";
import { useAssetSrc } from "./useAssetSrc";
import { basename } from "../../lib/format";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import { CodeEditor, type EditorSelection } from "./CodeEditor";
import { DiffView } from "./DiffView";
import { diffTab, diffTarget, isDiffPath } from "./diff";
import { useGitStatus } from "./useGitStatus";
import { rendererFor } from "./renderers";
import { setDragPaths } from "./dnd";
import { useFsChanged } from "./useFsChanged";
import "./files.css";

/** One key per tab, stable across reorders: the path group it shows. */
const tabKey = (tab: string[]) => tab.join("\n");

export function EditorPane({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const pane = useStore((s) => (s.session ? s.editorTabs[s.session.session_id] : undefined));
  const activateTab = useStore((s) => s.activateEditorTab);
  const closeTab = useStore((s) => s.closeEditorTab);
  const closeViewer = useStore((s) => s.closeViewer);
  // Keeps git status fresh even when the Files tree isn't mounted, so the
  // editor's "view diff" affordance appears/disappears with the file's state.
  useGitStatus(workspace);

  // Unsaved-edit state per tab key, reported up by each CodeView so the tab
  // strip can mark dirty tabs and closes can warn before discarding.
  const [dirtyTabs, setDirtyTabs] = useState<Record<string, boolean>>({});
  const reportDirty = useCallback((key: string, dirty: boolean) => {
    setDirtyTabs((m) => (!!m[key] === dirty ? m : { ...m, [key]: dirty }));
  }, []);

  if (!workspace || !pane?.tabs.length) return null;
  const { tabs, active } = pane;

  // Drafts belong to the shared document store, so closing a surface is safe.
  function requestCloseTab(index: number) {
    closeTab(index);
  }
  function requestClosePane() {
    closeViewer();
  }

  return (
    <aside className="canvas editor-pane">
      {onResizeStart && (
        <div
          className="canvas-resizer"
          onPointerDown={onResizeStart}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize editor"
        />
      )}
      {tabs.length > 1 && (
        <div className="canvas-head">
          <select
            aria-label="Open files"
            value={active}
            onChange={(e) => activateTab(Number(e.target.value))}
          >
            {tabs.map((tab, i) => (
              <option key={tabKey(tab)} value={i}>
                {basename(tab[0])}
                {dirtyTabs[tabKey(tab)] ? " · edited" : ""}
              </option>
            ))}
          </select>
          <button
            className="icon-btn sm"
            aria-label="Close current file"
            onClick={() => requestCloseTab(active)}
          >
            <X size={14} />
          </button>
        </div>
      )}
      {tabs.map((tab, i) => {
        const key = tabKey(tab);
        const single = tab.length === 1 ? tab[0] : null;
        let body;
        if (tab.length > 1) {
          body = <Gallery workspace={workspace} paths={tab} onClose={requestClosePane} />;
        } else if (single && isDiffPath(single)) {
          // Checked before extension sniffing: `diff:photo.png` is a diff.
          body = (
            <DiffView workspace={workspace} path={diffTarget(single)} onClose={requestClosePane} />
          );
        } else if (single && (isImagePath(single) || isVideoPath(single))) {
          body = <MediaView workspace={workspace} path={single} onClose={requestClosePane} />;
        } else if (single) {
          body = (
            <CodeView
              workspace={workspace}
              path={single}
              onClose={requestClosePane}
              onDirtyChange={(d) => reportDirty(key, d)}
            />
          );
        }
        return (
          <div key={`${workspace}:${key}`} className="editor-tab-body" hidden={i !== active}>
            {body}
          </div>
        );
      })}
    </aside>
  );
}

function CloseButton({ onClose }: { onClose: () => void }) {
  return (
    <button className="icon-btn sm" aria-label="Close editor" title="Close" onClick={onClose}>
      <X size={15} />
    </button>
  );
}

// ---- text files: the code editor --------------------------------------------

function CodeView({
  workspace,
  path,
  onClose,
  onDirtyChange,
}: {
  workspace: string;
  path: string;
  onClose: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const addSnippet = useStore((s) => s.addSnippet);
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  const openInViewer = useStore((s) => s.openInViewer);
  const wrap = useStore((s) => s.editorWrap);
  const toggleWrap = useStore((s) => s.toggleEditorWrap);
  /** Whether git says this file differs from HEAD — shows the diff jump. */
  const changed = useStore((s) => !!s.gitStates[workspace]?.some((g) => g.path === path));

  // Files with a registered rich renderer (markdown, html, …) get a
  // Preview/Raw toggle; the raw editor stays mounted underneath so unsaved
  // edits and undo history survive flipping views.
  const renderer = rendererFor(path);
  const [mode, setMode] = useState<"preview" | "raw">(renderer?.defaultMode ?? "raw");

  const session = useStore((s) => s.session?.session_id ?? "");
  const target = useMemo(() => ({ view: "editor", path }), [path]);
  const api = useWorkbenchAPI({ session, workspace, target });
  const document = useDocument(api, path);
  const [error, setError] = useState<string | null>(null);
  const [largePreview, setLargePreview] = useState<string>();
  const oversized = document.error?.includes("editable document limit") ?? false;
  useEffect(() => {
    if (!oversized) {
      setLargePreview(undefined);
      return;
    }
    let live = true;
    void fsReadFile(workspace, path)
      .then((body) => {
        if (live) setLargePreview(body.content);
      })
      .catch((reason) => {
        if (live) setError(String(reason));
      });
    return () => {
      live = false;
    };
  }, [workspace, path, oversized]);
  // What the editor shows: the shared draft (edits land there on every
  // keystroke, so it is also what the preview renders), or a read-only copy of
  // a file too large for the document store.
  const loaded = document.snapshot
    ? { doc: document.content, truncated: false }
    : largePreview !== undefined
      ? { doc: largePreview, truncated: true }
      : null;
  const dirty = document.dirty;
  const [justSaved, setJustSaved] = useState(false);
  const [selection, setSelection] = useState<EditorSelection | null>(null);
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);
  useEffect(() => {
    if (justSaved) {
      const timer = setTimeout(() => setJustSaved(false), 2000);
      return () => clearTimeout(timer);
    }
  }, [justSaved]);
  useEffect(() => {
    if (!running) void document.reload();
  }, [running]);
  async function save() {
    if (!dirty) return;
    try {
      await document.save();
      setError(null);
      setJustSaved(true);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <header className="canvas-head editor-head">
        <div className="editor-path" title={path}>
          <FileCode2 size={14} aria-hidden="true" />
          <span className="editor-fname">{basename(path)}</span>
          {dirty ? (
            <span className="editor-savestate is-edited" title="Unsaved changes — ⌘S to save">
              <span className="editor-savestate-dot" aria-hidden="true" />
              Edited
            </span>
          ) : justSaved ? (
            <span className="editor-savestate is-saved" role="status">
              <Check size={11} aria-hidden="true" />
              Saved
            </span>
          ) : null}
          {loaded?.truncated && <span className="editor-note">too large — read-only preview</span>}
        </div>
        <div className="editor-actions">
          {selection && mode === "raw" && (
            <button
              className="editor-tochat"
              title={`Send lines ${selection.start}-${selection.end} to the chat as context`}
              onClick={() => addSnippet({ path, ...selection })}
            >
              <MessageSquarePlus size={13} />
              <span>Add to chat</span>
            </button>
          )}
          {dirty && (
            <button
              className="icon-btn sm"
              aria-label="Save file"
              title="Save (⌘S)"
              onClick={() => void save()}
            >
              <Save size={14} />
            </button>
          )}
          {changed && (
            <button
              className="icon-btn sm"
              aria-label={`View diff of ${basename(path)}`}
              title="View diff"
              onClick={() => openInViewer([diffTab(path)])}
            >
              <FileDiff size={14} />
            </button>
          )}
          <button
            className={`icon-btn sm${wrap ? " active" : ""}`}
            aria-label="Toggle word wrap"
            aria-pressed={wrap}
            title={wrap ? "Unwrap long lines" : "Wrap long lines"}
            onClick={toggleWrap}
          >
            <WrapText size={14} />
          </button>
          {renderer && (
            <div className="editor-mode" role="group" aria-label="View mode">
              <button
                aria-pressed={mode === "preview"}
                className={mode === "preview" ? "active" : ""}
                onClick={() => setMode("preview")}
              >
                <Eye size={12} aria-hidden="true" />
                {renderer.label}
              </button>
              <button
                aria-pressed={mode === "raw"}
                className={mode === "raw" ? "active" : ""}
                onClick={() => setMode("raw")}
              >
                <Code2 size={12} aria-hidden="true" />
                Raw
              </button>
            </div>
          )}
          <CloseButton onClose={onClose} />
        </div>
      </header>
      {(error || (document.error && !largePreview)) && (
        <p className="editor-error" role="alert">
          {error || document.error}
        </p>
      )}
      {document.conflict && (
        <div className="workbench-conflict" role="alert">
          File changed on disk. Your edits are preserved.
          <button onClick={() => document.resolve("disk")}>Use disk version</button>
          <button onClick={() => document.resolve("draft")}>Keep my draft</button>
        </div>
      )}
      <div className="editor-body">
        {loaded &&
          renderer &&
          mode === "preview" &&
          renderer.render(loaded.doc, { workspace, path })}
        {loaded && (
          <div className="editor-raw" hidden={!!renderer && mode === "preview"}>
            <CodeEditor
              value={loaded.doc}
              filename={basename(path)}
              readOnly={loaded.truncated}
              wrap={wrap}
              onChange={document.edit}
              onSelection={setSelection}
              onSave={() => void save()}
            />
          </div>
        )}
      </div>
    </>
  );
}

// ---- one image or video ------------------------------------------------------

function MediaView({
  workspace,
  path,
  onClose,
}: {
  workspace: string;
  path: string;
  onClose: () => void;
}) {
  const abs = `${workspace}/${path}`;
  // The asset URL is stable, so a changed file would show its cached pixels —
  // bust the cache whenever the watcher sees this path rewritten.
  const [bust, setBust] = useState(0);
  useFsChanged(workspace, [path], () => setBust((b) => b + 1));
  const src = useAssetSrc(workspace, path, bust);
  const video = isVideoPath(path);
  return (
    <>
      <header className="canvas-head editor-head">
        <div className="editor-path" title={path}>
          {video ? (
            <Film size={14} aria-hidden="true" />
          ) : (
            <ImageIcon size={14} aria-hidden="true" />
          )}
          <span className="editor-fname">{basename(path)}</span>
        </div>
        <div className="editor-actions">
          <CloseButton onClose={onClose} />
        </div>
      </header>
      <div className="media-view">
        {src === null ? null : video ? (
          <video src={src} controls />
        ) : (
          <img
            src={src}
            alt={basename(path)}
            draggable
            onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [abs])}
            title="Drag into the chat to attach"
          />
        )}
      </div>
    </>
  );
}

// ---- several images: the gallery grid ---------------------------------------

function Gallery({
  workspace,
  paths,
  onClose,
}: {
  workspace: string;
  paths: string[];
  onClose: () => void;
}) {
  const images = paths.filter(isImagePath);
  // One shared cache-buster: a batch touching any tile refreshes the grid.
  const [bust, setBust] = useState(0);
  useFsChanged(workspace, images, () => setBust((b) => b + 1));
  return (
    <>
      <header className="canvas-head editor-head">
        <div className="editor-path">
          <ImageIcon size={14} aria-hidden="true" />
          <span className="editor-fname">{images.length} images</span>
        </div>
        <div className="editor-actions">
          <CloseButton onClose={onClose} />
        </div>
      </header>
      <div className="media-grid">
        {images.map((p) => (
          <GalleryTile key={p} workspace={workspace} path={p} bust={bust} />
        ))}
      </div>
      <p className="editor-hint">Drag a tile into the chat to attach it as context.</p>
    </>
  );
}

function GalleryTile({ workspace, path, bust }: { workspace: string; path: string; bust: number }) {
  const abs = `${workspace}/${path}`;
  const src = useAssetSrc(workspace, path, bust);
  if (!src) return null;
  return (
    <button
      className="media-tile"
      title={`${path} — drag into the chat to attach`}
      draggable
      onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [abs])}
    >
      <img src={src} alt={basename(path)} loading="lazy" draggable={false} />
      <span className="media-tile-name">{basename(path)}</span>
    </button>
  );
}
