import { useEffect, useState } from "react";
import "./hljs.css";

// highlight.js is heavy, so it's loaded on demand (and cached) the first time any
// code is highlighted — keeping it out of the app's startup bundle.
let hljsReady: Promise<typeof import("highlight.js").default> | null = null;
export function loadHljs() {
  if (!hljsReady) hljsReady = import("highlight.js").then((m) => m.default);
  return hljsReady;
}

/** Syntax-highlighted code via highlight.js. Renders the raw text immediately
 *  (so it's never blank) and re-highlights as `code` changes — keeping the prior
 *  highlighted markup until the new pass resolves, so streaming content updates
 *  without flicker. Wrap in your own <pre> for layout.
 *
 *  `autoDetect: false` renders plain text when `language` is missing or unknown
 *  instead of falling back to `highlightAuto` — detection tries every
 *  registered grammar, far too expensive to run repeatedly on streaming
 *  content. */
export function HighlightedCode({
  code,
  language,
  autoDetect = true,
}: {
  code: string;
  language?: string | null;
  autoDetect?: boolean;
}) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    if (!language && !autoDetect) {
      setHtml(null);
      return;
    }
    loadHljs()
      .then((hljs) => {
        if (!alive) return;
        try {
          const named = language && hljs.getLanguage(language) ? language : null;
          if (!named && !autoDetect) {
            setHtml(null);
            return;
          }
          const result = named
            ? hljs.highlight(code, { language: named })
            : hljs.highlightAuto(code);
          setHtml(result.value);
        } catch {
          setHtml(null);
        }
      })
      .catch(() => alive && setHtml(null));
    return () => {
      alive = false;
    };
  }, [code, language, autoDetect]);

  return html != null ? (
    <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
  ) : (
    <code className="hljs">{code}</code>
  );
}
