import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import "./markdown.css";

/** Render assistant text as GitHub-flavored Markdown in a `.prose` container.
 *  `components` optionally overrides element renderers (react-markdown's
 *  passthrough) — e.g. the skills pages decorate `code` spans that name tools. */
export function Markdown({ text, components }: { text: string; components?: Components }) {
  return (
    <div className="prose">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
