// The thread's trail, pinned inside its own chat: the same line the board
// draws — stations, wagon, settle ring — plus the tie-off, so a finished ride
// can be closed right where it ended instead of a trip back Home. Reads the
// same derived board as everything else; if this session has no entry yet
// (no user turn), there is nothing to pin.

import { useEffect, useState } from "react";
import { useStore } from "../../lib/store";
import { findThread } from "./ledger";
import { Trail } from "./Trail";
import { useBoard } from "./useBoard";
import { statusLine, TieKnot } from "./Wagon";
import "./ledger.css";

export function TrailStrip() {
  const sessionId = useStore((s) => s.session?.session_id);
  const refreshLedger = useStore((s) => s.refreshLedger);
  const reopenThread = useStore((s) => s.reopenThread);
  const [settling, setSettling] = useState(false);

  // The chat can be reached without ever visiting Home — the strip fetches
  // its own snapshot on mount and session switch. After that the store keeps
  // it fresh: turn end and landed update_plan/update_trail calls refresh the
  // ledger unconditionally (not gated on the board being open).
  useEffect(() => {
    void refreshLedger();
  }, [sessionId, refreshLedger]);

  const board = useBoard();
  const thread = board && sessionId ? findThread(board, sessionId) : null;
  if (!thread) return null;

  return (
    <div className="chat-trail" aria-label="This thread's trail">
      <Trail thread={thread} settling={settling} />
      <span className="chat-trail-status">{settling ? "tying off…" : statusLine(thread)}</span>
      {thread.state === "settled" ? (
        <button
          className="ledger-row-action"
          title="Untie — bring this thread back to the trail"
          onClick={() => void reopenThread(thread.entry.id)}
        >
          untie
        </button>
      ) : (
        thread.state !== "running" && (
          <TieKnot thread={thread} settling={settling} onSettling={setSettling} />
        )
      )}
    </div>
  );
}
