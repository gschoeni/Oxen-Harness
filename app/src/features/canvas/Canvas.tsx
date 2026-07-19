// The side-panel canvas: renders the document the agent showed via the `canvas`
// tool. One panel per chat; the agent updates a doc by re-sending the same id.
// Formats: markdown (rich), code (mono), and html/web & svg (sandboxed iframe —
// the content is model-authored, so it never gets same-origin access).

import { type PointerEvent } from "react";
import { X } from "lucide-react";
import { useStore } from "../../lib/store";
import { useThrottled } from "../../lib/useThrottled";
import { Markdown } from "../../components/ui/Markdown";
import { HighlightedCode } from "../../components/ui/HighlightedCode";
import type { CanvasDoc } from "../../lib/types";
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
            docs.map((d) => (
              <button
                key={d.id}
                className={`canvas-tab ${d.id === doc.id ? "active" : ""}`}
                onClick={() => setActiveCanvas(d.id)}
                title={d.title}
              >
                {d.title}
              </button>
            ))
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
          // Key on id+content so a switch or update fully remounts the view.
          <CanvasView key={`${committed.id}:${committed.content.length}`} doc={committed} />
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

function CanvasView({ doc }: { doc: CanvasDoc }) {
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
      // Committed docs are one-shot renders (keyed on id+content above), so
      // auto-detection for a missing language is a single affordable pass.
      return (
        <pre className="canvas-code hljs-theme">
          <HighlightedCode code={doc.content} language={doc.language} />
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
