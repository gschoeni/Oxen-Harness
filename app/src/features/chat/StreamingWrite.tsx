// Live preview of a file being written. While a write_file/edit_file tool call's
// arguments stream in, this shows the file content taking shape (syntax-
// highlighted by extension) so the user has immediate feedback that something is
// happening — before the call completes and the regular tool chip takes over.
//
// Deltas arrive far faster than a human reads: the raw stream is throttled to a
// ~8 fps repaint, only the visible tail of the file is rendered, and unknown
// languages render plain rather than paying for highlight.js auto-detection —
// re-highlighting an entire growing file on every delta is what froze the app.

import { useEffect, useMemo, useRef } from "react";
import { FilePlus2, PencilLine } from "lucide-react";
import { useStore } from "../../lib/store";
import { partialFileWrite } from "../../lib/streamingArgs";
import { useThrottled } from "../../lib/useThrottled";
import { HighlightedCode } from "../../components/ui/HighlightedCode";
import { basename } from "../../lib/format";
import "./streamingwrite.css";

/** How much of the in-progress file the preview renders. The view is pinned to
 *  the newest lines, so anything beyond this has scrolled out of sight. */
const MAX_PREVIEW_CHARS = 12_000;

/** The last `MAX_PREVIEW_CHARS` of `content`, trimmed to a line boundary. */
function visibleTail(content: string): string {
  if (content.length <= MAX_PREVIEW_CHARS) return content;
  const tail = content.slice(-MAX_PREVIEW_CHARS);
  const firstNewline = tail.indexOf("\n");
  return firstNewline >= 0 ? tail.slice(firstNewline + 1) : tail;
}

export function StreamingWrite() {
  const stream = useThrottled(
    useStore((s) => (s.session ? s.streamingTool[s.session.session_id] : undefined)),
    120,
  );
  const codeRef = useRef<HTMLPreElement>(null);
  const write = useMemo(
    () => (stream ? partialFileWrite(stream.name, stream.args) : null),
    [stream],
  );
  const content = write ? visibleTail(write.content) : "";

  // Keep the latest lines in view as content streams in.
  useEffect(() => {
    const el = codeRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [content]);

  // Only file-writing tools get an inline preview; other tools (and the idle
  // state) render nothing.
  if (!write) return null;

  const Icon = stream?.name === "edit_file" ? PencilLine : FilePlus2;
  const label = write.path ? basename(write.path) : "file";

  return (
    <div className="streaming-write">
      <div className="streaming-write-head">
        <Icon size={15} />
        <span className="streaming-write-verb">{write.verb}</span>
        <span className="streaming-write-path" title={write.path ?? undefined}>
          {label}
        </span>
        <span className="streaming-write-spinner" aria-label="writing" />
      </div>
      {content && (
        <pre className="streaming-write-code hljs-theme" ref={codeRef}>
          <HighlightedCode code={content} language={write.language} autoDetect={false} />
        </pre>
      )}
    </div>
  );
}
