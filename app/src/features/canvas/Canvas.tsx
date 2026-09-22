// The side-panel canvas: renders the document the agent showed via the `canvas`
// tool. One panel per chat; the agent updates a doc by re-sending the same id.
// Formats: markdown (rich), code (mono), and html/web & svg (sandboxed iframe —
// the content is model-authored, so it never gets same-origin access).

import { useEffect, useState, type PointerEvent } from "react";
import { X } from "lucide-react";
import { useStore } from "../../lib/store";
import { useThrottled } from "../../lib/useThrottled";
import { Markdown } from "../../components/ui/Markdown";
import { HighlightedCode } from "../../components/ui/HighlightedCode";
import type { CanvasDoc } from "../../lib/types";
import { useAssetSrc } from "../files/useAssetSrc";
import { useFsChanged } from "../files/useFsChanged";
import "./canvas.css";

export function Canvas({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const docs = useStore((s) => (s.session ? s.canvases[s.session.session_id] : undefined));
  const activeId = useStore((s) => (s.session ? s.activeCanvas[s.session.session_id] : undefined));
  const writing = useStore((s) => (s.session ? !!s.canvasWriting[s.session.session_id] : false));
  // The provisional doc updates on every streamed batch; rendering it (markdown
  // parse, highlighting) is expensive, so repaint at a human cadence instead.
  const streaming = useThrottled(
    useStore((s) => (s.session ? s.streamingCanvas[s.session.session_id] : undefined)),
    150,
  );
  const setActiveCanvas = useStore((s) => s.setActiveCanvas);
  const workspace = useStore((s) => s.session?.workspace ?? "");

  const committed = docs?.find((d) => d.id === activeId) ?? null;
  // Prefer the committed doc; while a new one is still being written, show the
  // provisional doc built from the streaming args so the panel fills in live.
  const doc = committed ?? (writing ? streaming ?? null : null);
  // The panel can be open with no doc yet (the model just started writing one).
  if (!doc && !writing) return null;

  return (
    <aside className="canvas">
      {onResizeStart && (
        <div
          className="canvas-resizer"
          onPointerDown={onResizeStart}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize canvas"
        />
      )}
      <header className="canvas-head">
        <div className="canvas-tabs">
          {doc && docs && docs.length > 1 ? (
            <select aria-label="Canvas document" value={doc.id} onChange={e=>setActiveCanvas(e.target.value)}>
              {docs.map(d=><option key={d.id} value={d.id}>{d.title}</option>)}
            </select>
          ) : (
            <span className="canvas-title">{doc ? doc.title : "Canvas"}</span>
          )}
          {doc && (
            <span className="canvas-badge">
              {doc.format}
              {doc.language ? ` · ${doc.language}` : ""}
            </span>
          )}
          {writing && <span className="canvas-writing">writing…</span>}
        </div>
        <button className="icon-btn" aria-label="Close canvas" onClick={() => setActiveCanvas(null)}>
          <X size={15} />
        </button>
      </header>
      <div className="canvas-body">
        {committed ? (
          committed.path && workspace ? (
            <MirroredCanvasView key={committed.id} doc={committed} workspace={workspace} />
          ) : (
            // Key on id+content so a switch or update fully remounts the view.
            <CanvasView key={`${committed.id}:${committed.content.length}`} doc={committed} />
          )
        ) : doc ? (
          // Still streaming: render the document in place as it forms.
          <CanvasStreamingView doc={doc} />
        ) : (
          <div className="canvas-placeholder">
            <span className="canvas-spinner" />
            <p>Writing document…</p>
          </div>
        )}
      </div>
    </aside>
  );
}

/** A document that mirrors a project file: the panel shows the file's text,
 *  read now and again whenever the watcher reports the file changed, so the
 *  agent's later edits land without another canvas call. Until the first
 *  read arrives (or when the file is out of reach), the content the call
 *  carried is shown. */
function MirroredCanvasView({ doc, workspace }: { doc: CanvasDoc; workspace: string }) {
  const path = doc.path ?? "";
  const [bust, setBust] = useState(0);
  useFsChanged(workspace, [path], () => setBust((b) => b + 1));
  const src = useAssetSrc(workspace, path, bust);
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    if (!src) return;
    let stale = false;
    fetch(src)
      .then((r) => (r.ok ? r.text() : Promise.reject(new Error(r.statusText))))
      .then((t) => {
        if (!stale) setText(t);
      })
      .catch(() => {
        /* unreadable right now: keep what we have */
      });
    return () => {
      stale = true;
    };
  }, [src]);
  const content = text ?? doc.content;
  return <CanvasView key={`${doc.id}:${content.length}:${bust}`} doc={{ ...doc, content }} />;
}

export function CanvasView({ doc }: { doc: CanvasDoc }) {
  switch (doc.format) {
    case "markdown":
      return (
        <div className="canvas-md">
          <Markdown text={doc.content} />
        </div>
      );
    case "html":
    case "svg":
      return <Sandboxed content={doc.content} />;
    case "code":
      // No auto-detection even for a committed doc: a canvas can be re-shown
      // and updated many times over a session, and `highlightAuto` runs every
      // registered grammar over the whole document. A doc that names no
      // language renders as plain text.
      return (
        <pre className="canvas-code hljs-theme">
          <HighlightedCode code={doc.content} language={doc.language} autoDetect={false} />
        </pre>
      );
    default:
      return (
        <pre className="canvas-code">
          <code>{doc.content}</code>
        </pre>
      );
  }
}

/** The document as it streams in (before it's committed). Markdown and code
 *  render live; html/svg show their source while writing, since rendering a
 *  half-written document would error or flicker — the formatted view appears
 *  the moment the call completes. */
function CanvasStreamingView({ doc }: { doc: CanvasDoc }) {
  if (doc.format === "markdown") {
    return (
      <div className="canvas-md">
        <Markdown text={doc.content} />
      </div>
    );
  }
  if (doc.format === "code") {
    return (
      <pre className="canvas-code hljs-theme">
        {/* No auto-detection mid-stream — it re-tries every grammar per repaint. */}
        <HighlightedCode code={doc.content} language={doc.language} autoDetect={false} />
      </pre>
    );
  }
  return (
    <pre className="canvas-code">
      <code>{doc.content}</code>
    </pre>
  );
}

/** Render model-authored HTML/SVG in a locked-down iframe: scripts may run, but
 *  without same-origin it can't touch the app's storage, cookies, or DOM. */
function Sandboxed({ content }: { content: string }) {
  return (
    <iframe
      className="canvas-frame"
      title="canvas document"
      sandbox="allow-scripts allow-popups allow-forms allow-modals"
      srcDoc={content}
    />
  );
}
