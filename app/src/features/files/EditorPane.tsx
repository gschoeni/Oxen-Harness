// The Editor dock: whatever the Files tree opened. One text file gets the
// CodeMirror editor (dirty tracking, ⌘S save, and "Add to chat" for the
// highlighted selection); an image or video renders natively via the asset
// protocol; a multi-selection of images becomes a gallery grid. Media can be
// dragged straight into the chat to become attachments.

import { useEffect, useRef, useState, type DragEvent, type PointerEvent } from "react";
import { FileCode2, Film, Image as ImageIcon, MessageSquarePlus, Save, X } from "lucide-react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useStore } from "../../lib/store";
import { fsReadFile, fsWriteFile } from "../../lib/ipc";
import { basename } from "../../lib/format";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import { CodeEditor, type EditorSelection } from "./CodeEditor";
import { setDragPaths } from "./dnd";
import "./files.css";

export function EditorPane({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const paths = useStore((s) => (s.session ? s.editorFiles[s.session.session_id] : undefined));
  const closeViewer = useStore((s) => s.closeViewer);

  if (!workspace || !paths?.length) return null;

  const single = paths.length === 1 ? paths[0] : null;
  let body;
  if (paths.length > 1) {
    body = <Gallery workspace={workspace} paths={paths} onClose={closeViewer} />;
  } else if (single && (isImagePath(single) || isVideoPath(single))) {
    body = <MediaView workspace={workspace} path={single} onClose={closeViewer} />;
  } else if (single) {
    body = <CodeView key={`${workspace}:${single}`} workspace={workspace} path={single} onClose={closeViewer} />;
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
      {body}
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

function CodeView({ workspace, path, onClose }: { workspace: string; path: string; onClose: () => void }) {
  const addSnippet = useStore((s) => s.addSnippet);
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");

  const [loaded, setLoaded] = useState<{ doc: string; truncated: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [selection, setSelection] = useState<EditorSelection | null>(null);
  const buffer = useRef("");

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

  async function save() {
    if (!dirty || loaded?.truncated) return;
    try {
      await fsWriteFile(workspace, path, buffer.current);
      setLoaded((prev) => (prev ? { ...prev, doc: buffer.current } : prev));
      setDirty(false);
      setError(null);
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
          {dirty && <span className="editor-dirty" title="Unsaved changes" />}
          {loaded?.truncated && <span className="editor-note">too large — read-only preview</span>}
        </div>
        <div className="editor-actions">
          {selection && (
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
          <CloseButton onClose={onClose} />
        </div>
      </header>
      {error && <p className="editor-error">{error}</p>}
      <div className="editor-body">
        {loaded && (
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
        )}
      </div>
    </>
  );
}

// ---- one image or video ------------------------------------------------------

function MediaView({ workspace, path, onClose }: { workspace: string; path: string; onClose: () => void }) {
  const abs = `${workspace}/${path}`;
  const src = convertFileSrc(abs);
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
              <img src={convertFileSrc(abs)} alt={basename(p)} loading="lazy" draggable={false} />
              <span className="media-tile-name">{basename(p)}</span>
            </button>
          );
        })}
      </div>
      <p className="editor-hint">Drag a tile into the chat to attach it as context.</p>
    </>
  );
}
