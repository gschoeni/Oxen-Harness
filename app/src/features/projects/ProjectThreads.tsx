// A project's chats on its home page, banded the way every chat list is:
// what needs you first, then the open chats, then the finished ones. Each
// row opens its chat; open rows can be marked finished, finished rows
// reopened. Reads the same verdicts the cards and the history do.

import { useMemo } from "react";
import { Check, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import type { SessionSummary } from "../../lib/types";
import { sectionRows } from "../history/sections";
import { StatusDot } from "../tabs/StatusDot";
import { tabSignals, tabStatus } from "../tabs/tabStatus";
import { useThreads } from "../threads/useThreads";

export function ProjectThreads({ workspace }: { workspace: string }) {
  const sessions = useStore((s) => s.sessions);
  const runStatus = useStore((s) => s.runStatus);
  const approvals = useStore((s) => s.approvals);
  const question = useStore((s) => s.question);
  const resume = useStore((s) => s.resume);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const finishThread = useStore((s) => s.finishThread);
  const reopenThread = useStore((s) => s.reopenThread);
  const threads = useThreads();

  // Imported transcripts are review-only and never resume as agents.
  const sections = useMemo(
    () =>
      sectionRows(
        sessions.filter((s) => s.source === "" && s.workspace === workspace),
        threads,
        runStatus,
      ),
    [sessions, workspace, threads, runStatus],
  );
  const total = sections.needs.length + sections.open.length + sections.finished.length;
  if (total === 0) return null;

  function open(id: string) {
    void resume(id).then(() => setHomeOpen(false));
  }

  const row = (s: SessionSummary, need?: string, finished = false) => {
    const status = tabStatus(tabSignals({ runStatus, approvals, question }, s.id, threads.get(s.id)));
    const running = status.state === "running";
    return (
      <div key={s.id} className={`project-thread ${finished ? "finished" : ""}`}>
        <button className="project-thread-open" onClick={() => open(s.id)}>
          <StatusDot state={status.state} label={status.label} />
          <span className="project-thread-text">
            <span className="project-thread-title">{s.title?.trim() || "New chat"}</span>
            {need && <span className="project-thread-need">{need}</span>}
          </span>
          <span className="project-thread-when">{relativeTime(s.created_at)}</span>
        </button>
        {finished ? (
          <Button size="sm" variant="ghost" className="project-thread-action" onClick={() => void reopenThread(s.id)}>
            <RotateCcw size={13} /> Reopen
          </Button>
        ) : (
          !running && (
            <Button
              size="sm"
              variant="outline"
              className="project-thread-action"
              title="Mark this chat finished"
              onClick={() => void finishThread(s.id)}
            >
              <Check size={13} /> Mark finished
            </Button>
          )
        )}
      </div>
    );
  };

  return (
    <section className="project-threads" aria-label="This project's chats">
      {sections.needs.length > 0 && (
        <h2 className="project-threads-head needs">
          Needs you <span className="project-threads-count">{sections.needs.length}</span>
        </h2>
      )}
      {sections.needs.map((n) => row(n.row, n.label))}
      {sections.open.length > 0 && (
        <h2 className="project-threads-head">{sections.needs.length > 0 ? "Other chats" : "Chats"}</h2>
      )}
      {sections.open.map((s) => row(s))}
      {sections.finished.length > 0 && (
        <h2 className="project-threads-head">
          Finished <span className="project-threads-count">{sections.finished.length}</span>
        </h2>
      )}
      {sections.finished.map((s) => row(s, undefined, true))}
    </section>
  );
}
