// The diff viewer tab: one changed file's unified diff (working tree vs
// HEAD), rendered with old/new line numbers and VS Code-style +/- coloring.
// It reloads itself whenever the file changes on disk or a turn ends, so it
// always shows the diff as it stands — including collapsing to "no changes"
// after a revert or a commit.

import { useEffect, useRef, useState } from "react";
import { FileCode2, FileDiff, WrapText, X } from "lucide-react";
import { gitDiff } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { basename } from "../../lib/format";
import { parseUnifiedDiff, type DiffRow } from "./diff";
import { useFsChanged } from "./useFsChanged";

export function DiffView({
  workspace,
  path,
  onClose,
}: {
  workspace: string;
  /** Workspace-relative path of the file whose diff is shown. */
  path: string;
  onClose: () => void;
}) {
  const openInViewer = useStore((s) => s.openInViewer);
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  const wrap = useStore((s) => s.editorWrap);
  const toggleWrap = useStore((s) => s.toggleEditorWrap);

  const [rows, setRows] = useState<DiffRow[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    try {
      const diff = await gitDiff(workspace, path);
      setRows(parseUnifiedDiff(diff.content));
      setTruncated(diff.truncated);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, path]);

  // The agent edits files during a turn; the diff must track the working tree.
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) void load();
    wasRunning.current = running;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);
  useFsChanged(workspace, [path], () => void load());

  // Straight into the hunks: the `diff --git`/`index`/`---`/`+++` preamble
  // repeats what the header already says (and `\ No newline` is noise here).
  const shown = rows?.filter((r) => r.kind !== "meta") ?? null;
  const empty = shown !== null && shown.length === 0;

  return (
    <>
      <header className="canvas-head editor-head">
        <div className="editor-path" title={`${path} — diff vs HEAD`}>
          <FileDiff size={14} aria-hidden="true" />
          <span className="editor-fname">{basename(path)}</span>
          <span className="editor-note">diff</span>
          {truncated && <span className="editor-note">too large — showing the start</span>}
        </div>
        <div className="editor-actions">
          <button
            className={`icon-btn sm${wrap ? " active" : ""}`}
            aria-label="Toggle word wrap"
            aria-pressed={wrap}
            title={wrap ? "Unwrap long lines" : "Wrap long lines"}
            onClick={toggleWrap}
          >
            <WrapText size={14} />
          </button>
          <button
            className="icon-btn sm"
            title="Open file"
            aria-label={`Open ${basename(path)}`}
            onClick={() => openInViewer([path])}
          >
            <FileCode2 size={14} />
          </button>
          <button className="icon-btn sm" aria-label="Close editor" title="Close" onClick={onClose}>
            <X size={15} />
          </button>
        </div>
      </header>
      {error && <p className="editor-error">{error}</p>}
      <div className="diff-view">
        {empty && <p className="dv-empty">No changes — this file matches HEAD.</p>}
        {shown !== null && !empty && (
          <div className={`dv-lines${wrap ? " wrap" : ""}`} role="table" aria-label={`Diff of ${path}`}>
            {shown.map((row, i) => (
              <div key={i} className={`dv-line dv-${row.kind}`} role="row">
                <span className="dv-no" role="cell">
                  {row.oldNo ?? ""}
                </span>
                <span className="dv-no" role="cell">
                  {row.newNo ?? ""}
                </span>
                <span className="dv-mark" aria-hidden="true">
                  {row.kind === "add" ? "+" : row.kind === "del" ? "-" : " "}
                </span>
                <span className="dv-text" role="cell">
                  {row.text}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
