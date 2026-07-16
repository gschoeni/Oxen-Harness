// Staged code snippets: selections highlighted in the editor (or whole files
// dropped from the tree) that ride along as context on the next prompt. The
// composer shows them as chips; `withSnippetContext` bakes them into the
// prompt text at send time, so queued prompts carry their context verbatim.

import type { CodeSnippet } from "./types";

/** Markdown fence hint for a filename — the extension is already the hint
 *  every renderer understands (```rs, ```tsx, ```py). */
export function fenceHint(path: string): string {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/** The chip label: `src/foo.ts:10-24` (single-line selections drop the range). */
export function snippetLabel(snippet: CodeSnippet): string {
  const range = snippet.end > snippet.start ? `${snippet.start}-${snippet.end}` : `${snippet.start}`;
  return `${snippet.path}:${range}`;
}

/** Prefix a prompt with each staged snippet as a cited, fenced block. */
export function withSnippetContext(text: string, snippets: CodeSnippet[]): string {
  if (!snippets.length) return text;
  const blocks = snippets.map(
    (snippet) =>
      `Context from \`${snippet.path}\` (lines ${snippet.start}-${snippet.end}):\n\n` +
      `\`\`\`${fenceHint(snippet.path)}\n${snippet.code}\n\`\`\``,
  );
  return [...blocks, text].join("\n\n");
}
