// Rich file renderers for the Editor pane. A registry maps file types to a
// special view (pretty markdown, rendered HTML, …) that the CodeView offers
// as a Preview/Raw toggle. Adding a renderer = one entry here: say which
// files it matches, how to render their text, and which side of the toggle
// a matching file opens on.

import type { ReactNode } from "react";
import { Markdown } from "../../components/ui/Markdown";

export interface FileRenderer {
  id: string;
  /** What the toggle calls the rich view (usually "Preview"). */
  label: string;
  /** Which view a matching file opens in. Reading-first formats (markdown)
   *  open pretty; source-first formats (html) open raw. */
  defaultMode: "preview" | "raw";
  matches: (path: string) => boolean;
  render: (content: string) => ReactNode;
}

const ext = (path: string) => path.split(".").pop()?.toLowerCase() ?? "";

export const RENDERERS: FileRenderer[] = [
  {
    id: "markdown",
    label: "Preview",
    defaultMode: "preview",
    matches: (p) => ["md", "markdown", "mdx"].includes(ext(p)),
    render: (content) => (
      <div className="editor-preview editor-preview-md">
        <Markdown text={content} />
      </div>
    ),
  },
  {
    id: "html",
    label: "Preview",
    defaultMode: "raw",
    matches: (p) => ["html", "htm"].includes(ext(p)),
    // Workspace HTML is still untrusted-ish (often model-authored): sandboxed,
    // so scripts may run but never with same-origin access to the app.
    render: (content) => (
      <iframe
        className="editor-preview-frame"
        title="HTML preview"
        sandbox="allow-scripts allow-popups allow-forms allow-modals"
        srcDoc={content}
      />
    ),
  },
];

/** The registered renderer for a path, or null when only raw text applies. */
export function rendererFor(path: string): FileRenderer | null {
  return RENDERERS.find((r) => r.matches(path)) ?? null;
}
