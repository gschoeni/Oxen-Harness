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
 *  without flicker. Wrap in your own <pre> for layout. */
export function HighlightedCode({
  code,
  language,
}: {
  code: string;
  language?: string | null;
}) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    loadHljs()
      .then((hljs) => {
        if (!alive) return;
        try {
          const result =
            language && hljs.getLanguage(language)
              ? hljs.highlight(code, { language })
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
  }, [code, language]);

  return html != null ? (
    <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
  ) : (
    <code className="hljs">{code}</code>
  );
}
