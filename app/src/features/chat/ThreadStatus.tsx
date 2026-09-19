// One line pinned atop the chat: where this thread stands, and the one human
// act — mark it finished (or reopen it). Reads the same verdict every other
// surface does; if this session has no entry yet (no user turn), there is
// nothing to pin.

import { useEffect, useState } from "react";
import { Check, Copy, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui";
import { relativeTime } from "../../lib/format";
import { useStore } from "../../lib/store";
import { useThreads } from "../threads/useThreads";
import type { Thread } from "../threads/threads";

export function ThreadStatus() {
  const sessionId = useStore((s) => s.session?.session_id);
  const refreshThreads = useStore((s) => s.refreshThreads);
  const finishThread = useStore((s) => s.finishThread);
  const reopenThread = useStore((s) => s.reopenThread);

  // The chat can be reached without ever visiting Home — refresh on mount
  // and on session switch; after that every turn end refreshes for us.
  useEffect(() => {
    void refreshThreads();
  }, [sessionId, refreshThreads]);

  const thread = useThreads().get(sessionId ?? "");
  if (!thread) return null;

  return (
    <div className="chat-status" aria-label="This chat's standing">
      <SessionIdChip id={thread.entry.id} />
      <span className="chat-status-text">{statusLine(thread)}</span>
      {thread.state === "finished" ? (
        <Button size="sm" variant="ghost" onClick={() => void reopenThread(thread.entry.id)}>
          <RotateCcw size={13} /> Reopen
        </Button>
      ) : (
        thread.state !== "running" && (
          <Button size="sm" variant="outline" onClick={() => void finishThread(thread.entry.id)}>
            <Check size={13} /> Mark finished
          </Button>
        )
      )}
    </div>
  );
}

/** Where the thread stands, in a few words. */
export function statusLine(thread: Thread): string {
  const when = relativeTime(thread.entry.last_activity_at);
  switch (thread.state) {
    case "running":
      return thread.stuck ? "waiting on your approval" : "running";
    case "dangling":
      return `reply never arrived · ${when}`;
    case "idle":
      return `last reply ${when}`;
    case "finished":
      return `finished · ${relativeTime(thread.entry.finished_at)}`;
  }
}

/** The session id, tail-clipped, copied to the clipboard on click. */
export function SessionIdChip({ id }: { id: string }) {
  const [copied, setCopied] = useState(false);
  const short = id.length > 12 ? `…${id.slice(-8)}` : id;
  return (
    <button
      type="button"
      className={`chat-session-id ${copied ? "copied" : ""}`}
      title={copied ? "Copied" : `Copy session id ${id}`}
      aria-label={copied ? "Session id copied" : `Copy session id ${id}`}
      onClick={(e) => {
        e.stopPropagation();
        void navigator.clipboard?.writeText(id).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1200);
        });
      }}
    >
      {copied ? <Check size={11} /> : <Copy size={11} />}
      <span>{copied ? "copied" : short}</span>
    </button>
  );
}
