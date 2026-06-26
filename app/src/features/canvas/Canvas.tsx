// The side-panel canvas: renders the document the agent showed via the `canvas`
// tool. One panel per chat; the agent updates a doc by re-sending the same id.
// Formats: markdown (rich), code (mono), html/web & svg (sandboxed iframe — the
// content is model-authored, so it never gets same-origin access), and mermaid
// (rendered to SVG by the bundled library).

import { useEffect, useState, type PointerEvent } from "react";
import { X } from "lucide-react";
import { useStore } from "../../lib/store";
import { Markdown } from "../../components/ui/Markdown";
import { HighlightedCode } from "../../components/ui/HighlightedCode";
import type { CanvasDoc } from "../../lib/types";
import "./canvas.css";

// Mermaid is heavy, so it's loaded on demand (and initialized once) the first
// time a diagram is rendered, keeping it out of the app's startup bundle.
let mermaidReady: Promise<typeof import("mermaid").default> | null = null;
function loadMermaid() {
  if (!mermaidReady) {
    mermaidReady = import("mermaid").then(({ default: m }) => {
      m.initialize({ startOnLoad: false, securityLevel: "strict", theme: "neutral" });
      return m;
    });
  }
  return mermaidReady;
}

// highlight.js is likewise loaded on demand, only when a code document renders.
let hljsReady: Promise<typeof import("highlight.js").default> | null = null;
function loadHljs() {
  if (!hljsReady) hljsReady = import("highlight.js").then((m) => m.default);
  return hljsReady;
}

export function Canvas({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const docs = useStore((s) => (s.session ? s.canvases[s.session.session_id] : undefined));
  const activeId = useStore((s) => (s.session ? s.activeCanvas[s.session.session_id] : undefined));
  const writing = useStore((s) => (s.session ? !!s.canvasWriting[s.session.session_id] : false));
  const streaming = useStore((s) => (s.session ? s.streamingCanvas[s.session.session_id] : undefined));
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
          <X size={16} />
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
    case "mermaid":
      return <MermaidView code={doc.content} id={doc.id} />;
    case "code":
      return <CodeView content={doc.content} language={doc.language} />;
    default:
      return (
        <pre className="canvas-code">
          <code>{doc.content}</code>
        </pre>
      );
  }
}

/** The document as it streams in (before it's committed). Markdown and code
 *  render live; html/svg/mermaid show their source while writing, since rendering
 *  a half-written document would error or flicker — the formatted view appears
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
      <pre className="canvas-code">
        <HighlightedCode code={doc.content} language={doc.language} />
      </pre>
    );
  }
  return (
    <pre className="canvas-code">
      <code>{doc.content}</code>
    </pre>
  );
}

/** Syntax-highlighted code (highlighting is theme-styled via canvas.css). Shows
 *  the raw text until the highlighter finishes loading, so it's never blank. */
function CodeView({ content, language }: { content: string; language?: string | null }) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setHtml(null);
    loadHljs()
      .then((hljs) => {
        if (!alive) return;
        const result =
          language && hljs.getLanguage(language)
            ? hljs.highlight(content, { language })
            : hljs.highlightAuto(content);
        setHtml(result.value);
      })
      .catch(() => alive && setHtml(null));
    return () => {
      alive = false;
    };
  }, [content, language]);

  return (
    <pre className="canvas-code">
      {html != null ? (
        <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
      ) : (
        <code className="hljs">{content}</code>
      )}
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

function MermaidView({ code, id }: { code: string; id: string }) {
  const [svg, setSvg] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setError(null);
    // A fresh render id avoids collisions with mermaid's temp DOM node.
    const renderId = `mmd-${id}-${code.length}`.replace(/[^a-zA-Z0-9_-]/g, "");
    loadMermaid()
      .then((m) => m.render(renderId, code))
      .then(({ svg }) => alive && setSvg(svg))
      .catch((e) => alive && setError(String(e?.message ?? e)));
    return () => {
      alive = false;
    };
  }, [code, id]);

  if (error) {
    return (
      <pre className="canvas-code canvas-error">
        <code>{error}</code>
      </pre>
    );
  }
  // mermaid output is sanitized (securityLevel: "strict").
  return <div className="canvas-mermaid" dangerouslySetInnerHTML={{ __html: svg }} />;
}
