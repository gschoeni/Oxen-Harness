// The Editor dock: whatever the Files tree opened, as a strip of tabs. Each
// tab is one text file (CodeMirror editor with dirty tracking, ⌘S save, and
// "Add to chat" for the highlighted selection), one image or video rendered
// natively via the asset protocol, or a multi-selection of images shown as a
// gallery grid. Every tab stays mounted so unsaved edits survive switching.
// Media can be dragged straight into the chat to become attachments.

import { useCallback, useEffect, useRef, useState, type DragEvent, type PointerEvent } from "react";
import {
  Check,
  Code2,
  Eye,
  FileCode2,
  Film,
  Image as ImageIcon,
  Images,
  MessageSquarePlus,
  Save,
  X,
} from "lucide-react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useStore } from "../../lib/store";
import { fsReadFile, fsWriteFile } from "../../lib/ipc";
import { basename } from "../../lib/format";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import { CodeEditor, type EditorSelection } from "./CodeEditor";
import { rendererFor } from "./renderers";
import { setDragPaths } from "./dnd";
import "./files.css";

/** One key per tab, stable across reorders: the path group it shows. */
const tabKey = (tab: string[]) => tab.join("\n");

/** Run `onChange` when a watcher batch touches any of `paths` in this
 *  workspace (an empty batch means "bulk change" and always matches). */
function useFsChanged(workspace: string, paths: string[], onChange: () => void) {
  const fsChange = useStore((s) => s.fsChange);
  useEffect(() => {
    if (!fsChange || fsChange.root !== workspace) return;
    if (fsChange.paths.length && !paths.some((p) => fsChange.paths.includes(p))) return;
    onChange();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fsChange]);
}

export function EditorPane({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const pane = useStore((s) => (s.session ? s.editorTabs[s.session.session_id] : undefined));
  const activateTab = useStore((s) => s.activateEditorTab);
  const closeTab = useStore((s) => s.closeEditorTab);
  const closeViewer = useStore((s) => s.closeViewer);

  // Unsaved-edit state per tab key, reported up by each CodeView so the tab
  // strip can mark dirty tabs and closes can warn before discarding.
  const [dirtyTabs, setDirtyTabs] = useState<Record<string, boolean>>({});
  const reportDirty = useCallback((key: string, dirty: boolean) => {
    setDirtyTabs((m) => (!!m[key] === dirty ? m : { ...m, [key]: dirty }));
  }, []);

  if (!workspace || !pane?.tabs.length) return null;
  const { tabs, active } = pane;

  function requestCloseTab(index: number) {
    const key = tabKey(tabs[index]);
    if (dirtyTabs[key] && !window.confirm(`Discard unsaved changes to ${basename(tabs[index][0])}?`)) return;
    setDirtyTabs((m) => {
      const next = { ...m };
      delete next[key];
      return next;
    });
    closeTab(index);
  }

  function requestClosePane() {
    const unsaved = tabs.filter((t) => dirtyTabs[tabKey(t)]);
    if (
      unsaved.length &&
      !window.confirm(
        unsaved.length === 1
          ? `Discard unsaved changes to ${basename(unsaved[0][0])}?`
          : `Discard unsaved changes in ${unsaved.length} files?`
      )
    )
      return;
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
        <div className="editor-tabs" role="tablist" aria-label="Open files">
          {tabs.map((tab, i) => (
            <Tab
              key={tabKey(tab)}
              tab={tab}
              active={i === active}
              dirty={!!dirtyTabs[tabKey(tab)]}
              onActivate={() => activateTab(i)}
              onClose={() => requestCloseTab(i)}
            />
          ))}
        </div>
      )}
      {tabs.map((tab, i) => {
        const key = tabKey(tab);
        const single = tab.length === 1 ? tab[0] : null;
        let body;
        if (tab.length > 1) {
          body = <Gallery workspace={workspace} paths={tab} onClose={requestClosePane} />;
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

// ---- one tab in the strip ----------------------------------------------------

function Tab({
  tab,
  active,
  dirty,
  onActivate,
  onClose,
}: {
  tab: string[];
  active: boolean;
  dirty: boolean;
  onActivate: () => void;
  onClose: () => void;
}) {
  const gallery = tab.length > 1;
  const path = tab[0];
  const name = gallery ? `${tab.length} images` : basename(path);
  const icon = gallery ? (
    <Images size={12} aria-hidden="true" />
  ) : isVideoPath(path) ? (
    <Film size={12} aria-hidden="true" />
  ) : isImagePath(path) ? (
    <ImageIcon size={12} aria-hidden="true" />
  ) : (
    <FileCode2 size={12} aria-hidden="true" />
  );
  return (
    <div
      className={`editor-tab${active ? " active" : ""}${dirty ? " dirty" : ""}`}
      role="tab"
      aria-selected={active}
      title={gallery ? tab.join("\n") : path}
      onClick={onActivate}
      onAuxClick={(e) => {
        // Middle-click closes, like every tabbed editor.
        if (e.button === 1) onClose();
      }}
    >
      {icon}
      <span className="editor-tab-name">{name}</span>
      <button
        className="editor-tab-close"
        aria-label={dirty ? `Close ${name} (unsaved changes)` : `Close ${name}`}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        <span className="editor-tab-dot" aria-hidden="true" />
        <X size={12} aria-hidden="true" />
      </button>
    </div>
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

  // Files with a registered rich renderer (markdown, html, …) get a
  // Preview/Raw toggle; the raw editor stays mounted underneath so unsaved
  // edits and undo history survive flipping views.
  const renderer = rendererFor(path);
  const [mode, setMode] = useState<"preview" | "raw">(renderer?.defaultMode ?? "raw");

  const [loaded, setLoaded] = useState<{ doc: string; truncated: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  // Briefly true after a successful save so the header can confirm it landed.
  const [justSaved, setJustSaved] = useState(false);
  const [selection, setSelection] = useState<EditorSelection | null>(null);
  const buffer = useRef("");
  const savedTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    onDirtyChange?.(dirty);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty]);
  useEffect(() => () => window.clearTimeout(savedTimer.current), []);

  async function load() {
    try {
      const body = await fsReadFile(workspace, path);
      buffer.current = body.content;
      setLoaded({ doc: body.content, truncated: body.truncated });
      setDirty(false);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, path]);

  // The agent may have rewritten this very file during its turn: pick up the
  // new content when the turn ends — but never over unsaved edits.
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running && !dirty) void load();
    wasRunning.current = running;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  // Any process rewrote this file on disk: reload — but unsaved edits win
  // (the user's buffer is never clobbered; saving overwrites the disk copy).
  // Our own saves echo back here as a same-content reload, which is free:
  // CodeMirror only rebuilds when the loaded text actually differs.
  useFsChanged(workspace, [path], () => {
    if (!dirty) void load();
  });

  async function save() {
    if (!dirty || loaded?.truncated) return;
    try {
      await fsWriteFile(workspace, path, buffer.current);
      setLoaded((prev) => (prev ? { ...prev, doc: buffer.current } : prev));
      setDirty(false);
      setError(null);
      setJustSaved(true);
      window.clearTimeout(savedTimer.current);
      savedTimer.current = window.setTimeout(() => setJustSaved(false), 2000);
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
            <button className="icon-btn sm" aria-label="Save file" title="Save (⌘S)" onClick={() => void save()}>
              <Save size={14} />
            </button>
          )}
          {renderer && (
            <div className="editor-mode" role="tablist" aria-label="View mode">
              <button
                role="tab"
                aria-selected={mode === "preview"}
                className={mode === "preview" ? "active" : ""}
                onClick={() => setMode("preview")}
              >
                <Eye size={12} aria-hidden="true" />
                {renderer.label}
              </button>
              <button
                role="tab"
                aria-selected={mode === "raw"}
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
      {error && <p className="editor-error">{error}</p>}
      <div className="editor-body">
        {loaded && renderer && mode === "preview" && renderer.render(dirty ? buffer.current : loaded.doc)}
        {loaded && (
          <div className="editor-raw" hidden={!!renderer && mode === "preview"}>
            <CodeEditor
              initial={loaded.doc}
              filename={basename(path)}
              readOnly={loaded.truncated}
              onChange={(doc) => {
                buffer.current = doc;
                setDirty(doc !== loaded.doc);
              }}
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

function MediaView({ workspace, path, onClose }: { workspace: string; path: string; onClose: () => void }) {
  const abs = `${workspace}/${path}`;
  // The asset URL is stable, so a changed file would show its cached pixels —
  // bust the cache whenever the watcher sees this path rewritten.
  const [bust, setBust] = useState(0);
  useFsChanged(workspace, [path], () => setBust((b) => b + 1));
  const src = convertFileSrc(abs) + (bust ? `?v=${bust}` : "");
  const video = isVideoPath(path);
  return (
    <>
      <header className="canvas-head editor-head">
        <div className="editor-path" title={path}>
          {video ? <Film size={14} aria-hidden="true" /> : <ImageIcon size={14} aria-hidden="true" />}
          <span className="editor-fname">{basename(path)}</span>
        </div>
        <div className="editor-actions">
          <CloseButton onClose={onClose} />
        </div>
      </header>
      <div className="media-view">
        {video ? (
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
        {images.map((p) => {
          const abs = `${workspace}/${p}`;
          return (
            <button
              key={p}
              className="media-tile"
              title={`${p} — drag into the chat to attach`}
              draggable
              onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [abs])}
            >
              <img
                src={convertFileSrc(abs) + (bust ? `?v=${bust}` : "")}
                alt={basename(p)}
                loading="lazy"
                draggable={false}
              />
              <span className="media-tile-name">{basename(p)}</span>
            </button>
          );
        })}
      </div>
      <p className="editor-hint">Drag a tile into the chat to attach it as context.</p>
    </>
  );
}
