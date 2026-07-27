// The markdown Preview's workspace-aware rendering: embedded HTML rendered
// (sanitized, GitHub-style), local images resolved through the asset
// protocol, and repo-relative links opened in the editor pane instead of
// being shipped to the link browser as dead localhost URLs.

import { useMemo } from "react";
import type { Components } from "react-markdown";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Markdown } from "../../components/ui/Markdown";
import { useStore } from "../../lib/store";

/** A URL's workspace resolution: the file it points at, plus any fragment. */
export interface WorkspaceTarget {
  /** Workspace-relative path, `/`-joined and `.`/`..`-normalized. */
  rel: string;
  fragment: string;
}

/** The fragment of any URL ("" when none). */
export const urlFragment = (url: string): string => url.split("#")[1] ?? "";

/** Resolve a markdown `src`/`href` against the previewed file's directory.
 *
 *  Returns `null` for anything that isn't a workspace file: absolute URLs
 *  (any scheme, protocol-relative `//`), and pure fragments. Root-relative
 *  paths (`/images/logo.svg`) resolve against the workspace root — GitHub's
 *  behavior for repo READMEs — and `..` that escapes the root is refused. */
export function resolveWorkspaceUrl(dir: string, url: string): WorkspaceTarget | null {
  if (!url || url.startsWith("#") || url.startsWith("//")) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(url)) return null;
  const fragment = urlFragment(url);
  const pathPart = url.split("#")[0].split("?")[0];
  if (!pathPart) return null;
  const base = pathPart.startsWith("/") ? "" : dir;
  const joined = pathPart.startsWith("/") ? pathPart.slice(1) : base ? `${base}/${pathPart}` : pathPart;
  const parts: string[] = [];
  for (const seg of joined.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (!parts.length) return null; // escapes the workspace
      parts.pop();
      continue;
    }
    parts.push(seg);
  }
  if (!parts.length) return null;
  return { rel: parts.join("/"), fragment };
}

const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");

/** The rendered markdown Preview for one workspace file. */
export function MarkdownPreview({
  workspace,
  path,
  content,
}: {
  workspace: string;
  /** Workspace-relative path of the file being previewed. */
  path: string;
  content: string;
}) {
  const openInViewer = useStore((s) => s.openInViewer);
  const mode = useStore((s) => s.mode);
  const dir = parentOf(path);

  const components = useMemo<Components>(
    () => ({
      img({ node: _node, src, alt, ...rest }) {
        const url = typeof src === "string" ? src : "";
        // GitHub renders one of a #gh-dark-mode-only/#gh-light-mode-only pair
        // per theme; follow the app's mode instead of showing both broken.
        const fragment = urlFragment(url);
        if (fragment === "gh-dark-mode-only" && mode !== "dark") return null;
        if (fragment === "gh-light-mode-only" && mode !== "light") return null;
        const resolved = resolveWorkspaceUrl(dir, url);
        const finalSrc = resolved ? convertFileSrc(`${workspace}/${resolved.rel}`) : url;
        return <img src={finalSrc} alt={alt} loading="lazy" {...rest} />;
      },
      a({ node: _node, href, children, ...rest }) {
        const resolved = resolveWorkspaceUrl(dir, typeof href === "string" ? href : "");
        if (!resolved) {
          // External/fragment links keep their href; the app-wide interceptor
          // routes http(s) to the link browser and other schemes to the OS.
          return (
            <a href={href} {...rest}>
              {children}
            </a>
          );
        }
        // A workspace file: open it in the editor pane. Deliberately no href —
        // the document-level capture interceptor would otherwise grab the
        // click first and ship a dead localhost URL to the link browser.
        return (
          <a
            role="link"
            tabIndex={0}
            title={resolved.rel}
            onClick={() => openInViewer([resolved.rel])}
            onKeyDown={(e) => {
              if (e.key === "Enter") openInViewer([resolved.rel]);
            }}
            {...rest}
          >
            {children}
          </a>
        );
      },
    }),
    [dir, workspace, mode, openInViewer],
  );

  return (
    <div className="editor-preview editor-preview-md">
      <Markdown text={content} html components={components} />
    </div>
  );
}
