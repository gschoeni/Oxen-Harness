import { useEffect, useState } from "react";
import "./hljs.css";

/** The grammars the app bundles, each as its own lazy chunk. Explicit thunks
 *  (not a templated `import()`): Vite only code-splits dynamic imports it can
 *  see statically, and a bare-specifier template would either fail to resolve
 *  or pull the whole `lib/languages` directory into one chunk.
 *
 *  The set is every language `langForPath` can name (pinned by a test against
 *  `PATH_LANGUAGES`) plus what shows up in chat replies regardless of any
 *  file: shells, config formats, diffs, SQL, Dockerfiles, markup. That is ~30
 *  grammars instead of highlight.js's 192 — the full build was a ~970 KB chunk
 *  the app kept resident from the first highlighted line on. */
export const GRAMMARS: Record<string, () => Promise<{ default: import("highlight.js").LanguageFn }>> = {
  bash: () => import("highlight.js/lib/languages/bash"),
  c: () => import("highlight.js/lib/languages/c"),
  cpp: () => import("highlight.js/lib/languages/cpp"),
  csharp: () => import("highlight.js/lib/languages/csharp"),
  css: () => import("highlight.js/lib/languages/css"),
  diff: () => import("highlight.js/lib/languages/diff"),
  dockerfile: () => import("highlight.js/lib/languages/dockerfile"),
  go: () => import("highlight.js/lib/languages/go"),
  ini: () => import("highlight.js/lib/languages/ini"),
  java: () => import("highlight.js/lib/languages/java"),
  javascript: () => import("highlight.js/lib/languages/javascript"),
  json: () => import("highlight.js/lib/languages/json"),
  kotlin: () => import("highlight.js/lib/languages/kotlin"),
  less: () => import("highlight.js/lib/languages/less"),
  markdown: () => import("highlight.js/lib/languages/markdown"),
  php: () => import("highlight.js/lib/languages/php"),
  plaintext: () => import("highlight.js/lib/languages/plaintext"),
  python: () => import("highlight.js/lib/languages/python"),
  ruby: () => import("highlight.js/lib/languages/ruby"),
  rust: () => import("highlight.js/lib/languages/rust"),
  scala: () => import("highlight.js/lib/languages/scala"),
  scss: () => import("highlight.js/lib/languages/scss"),
  shell: () => import("highlight.js/lib/languages/shell"),
  sql: () => import("highlight.js/lib/languages/sql"),
  swift: () => import("highlight.js/lib/languages/swift"),
  typescript: () => import("highlight.js/lib/languages/typescript"),
  xml: () => import("highlight.js/lib/languages/xml"),
  yaml: () => import("highlight.js/lib/languages/yaml"),
};

/** Aliases the chat commonly names that no bundled grammar declares itself.
 *  (Most of the usual ones — js/ts/py/sh/rs/yml/md/html/toml/… — come from the
 *  grammar files' own `aliases`, so registering the grammar is enough.) */
const EXTRA_ALIASES: Record<string, string[]> = {
  shell: ["zsh", "fish"],
  bash: ["shell-script"],
  xml: ["vue"],
  json: ["jsonc", "json5"],
  plaintext: ["log"],
};

// highlight.js's core is loaded on demand (and cached) the first time any code
// is highlighted, with the curated grammars registered into it in the same
// step — nothing of it sits in the startup bundle, and only the grammar chunks
// listed above are ever fetched.
let hljsReady: Promise<typeof import("highlight.js/lib/core").default> | null = null;
export function loadHljs() {
  if (!hljsReady) {
    hljsReady = Promise.all([
      import("highlight.js/lib/core"),
      ...Object.entries(GRAMMARS).map(async ([name, load]) => [name, (await load()).default] as const),
    ]).then(([core, ...grammars]) => {
      const hljs = core.default;
      for (const [name, fn] of grammars) {
        hljs.registerLanguage(name, fn);
        const extra = EXTRA_ALIASES[name];
        if (extra) hljs.registerAliases(extra, { languageName: name });
      }
      return hljs;
    });
  }
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
