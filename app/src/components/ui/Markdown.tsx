import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import "./markdown.css";

/** Render assistant text as GitHub-flavored Markdown in a `.prose` container. */
export function Markdown({ text }: { text: string }) {
  return (
    <div className="prose">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
