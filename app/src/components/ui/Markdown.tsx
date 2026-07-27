import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import "./markdown.css";

type RehypePlugins = NonNullable<Parameters<typeof ReactMarkdown>[0]["rehypePlugins"]>;

// GitHub's sanitize schema, widened just enough for README chrome: `align`
// on the block elements badge headers center with. Scripts, event handlers,
// and javascript: URLs stay stripped — raw HTML renders, but inert.
const schema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    div: [...(defaultSchema.attributes?.div ?? []), "align"],
    p: [...(defaultSchema.attributes?.p ?? []), "align"],
  },
};

// rehype-raw must run before rehype-sanitize: parse the embedded HTML first,
// then clean the combined tree.
const htmlPlugins: RehypePlugins = [rehypeRaw, [rehypeSanitize, schema]];

/** Render assistant text as GitHub-flavored Markdown in a `.prose` container.
 *  `components` optionally overrides element renderers (react-markdown's
 *  passthrough) — e.g. the skills pages decorate `code` spans that name tools.
 *  `html` additionally renders raw HTML embedded in the markdown (sanitized,
 *  GitHub-style) — for trusted-ish sources like workspace READMEs, NOT chat. */
export function Markdown({
  text,
  components,
  html = false,
}: {
  text: string;
  components?: Components;
  html?: boolean;
}) {
  return (
    <div className="prose">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={html ? htmlPlugins : undefined}
        components={components}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
