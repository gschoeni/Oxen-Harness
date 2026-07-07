// Live preview of a file being written. While a write_file/edit_file tool call's
// arguments stream in, this shows the file content taking shape (syntax-
// highlighted by extension) so the user has immediate feedback that something is
// happening — before the call completes and the regular tool chip takes over.

import { useEffect, useRef } from "react";
import { FilePlus2, PencilLine } from "lucide-react";
import { useStore } from "../../lib/store";
import { partialFileWrite } from "../../lib/streamingArgs";
import { HighlightedCode } from "../../components/ui/HighlightedCode";
import { basename } from "../../lib/format";
import "./streamingwrite.css";

export function StreamingWrite() {
  const stream = useStore((s) => (s.session ? s.streamingTool[s.session.session_id] : undefined));
  const codeRef = useRef<HTMLPreElement>(null);
  const write = stream ? partialFileWrite(stream.name, stream.args) : null;

  // Keep the latest lines in view as content streams in.
  useEffect(() => {
    const el = codeRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [write?.content]);

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
      {write.content && (
        <pre className="streaming-write-code hljs-theme" ref={codeRef}>
          <HighlightedCode code={write.content} language={write.language} />
        </pre>
      )}
    </div>
  );
}
