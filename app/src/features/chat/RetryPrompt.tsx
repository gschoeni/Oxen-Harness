import { useEffect, useState } from "react";
import { CreditCard, ExternalLink, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui";
import { getConnection } from "../../lib/ipc";
import { isCreditsError, useStore } from "../../lib/store";
import type { Item } from "./thread";
import { ModelErrorModal } from "./ModelError";
import "./apikey.css";

type RetryItem = Extract<Item, { kind: "retry" }>;

/** Inline recovery for a turn that died recoverably: an out-of-credits (402)
 *  failure, or a chat resumed from history whose transcript stopped mid-turn.
 *  Shown where the reply would be; one click re-drives the failed turn against
 *  the existing transcript, so the conversation continues in place. */
export function RetryPrompt({ item }: { item: RetryItem }) {
  const sessionId = useStore((s) => s.session?.session_id);
  const running = useStore((s) => (sessionId ? s.runStatus[sessionId] === "running" : false));
  const retryBrokenTurn = useStore((s) => s.retryBrokenTurn);

  const credits = isCreditsError(item.message);
  // Where to add credits — the connected hub (or a custom/self-hosted host).
  const [host, setHost] = useState("");
  useEffect(() => {
    if (!credits) return;
    getConnection()
      .then((c) => setHost(c.host || c.default_host))
      .catch(() => {});
  }, [credits]);
  const hubUrl = /^https?:\/\//.test(host) ? host : `https://${host || "hub.oxen.ai"}`;
  // The card's one line summarises the turn's end; the detail view behind it
  // has the last provider response, status, model and endpoint for debugging.
  const [details, setDetails] = useState(false);

  return (
    <div className="apikey-card">
      <div className="apikey-head">
        <span className="apikey-icon">
          {credits ? <CreditCard size={15} /> : <RotateCcw size={15} />}
        </span>
        <div className="apikey-head-text">
          <div className="apikey-title">
            {credits ? "Out of Oxen credits" : "Continue this chat"}
          </div>
          {/* The title already says what a 402 means, so the raw provider
              line stays behind "View error details". */}
          <div className="apikey-sub">
            {credits ? (
              <>
                Add credits at <code className="apikey-host">{host || "hub.oxen.ai"}</code>, then
                retry. This chat picks up right where it left off.
              </>
            ) : (
              `${item.message} Nothing is lost — retry as-is, or switch models or check your connection first, then pick up right where you left off.`
            )}
          </div>
        </div>
      </div>

      <div className="retry-actions">
        {credits && (
          <a className="btn primary sm" href={`${hubUrl}/settings`} target="_blank" rel="noreferrer">
            Add credits
            <ExternalLink size={14} />
          </a>
        )}
        <Button
          variant={credits ? "outline" : "primary"}
          size="sm"
          disabled={!sessionId || running}
          onClick={() => sessionId && retryBrokenTurn(sessionId, item.id)}
        >
          <RotateCcw size={14} />
          {credits ? "Retry" : "Continue"}
        </Button>
        <button type="button" className="retry-details" onClick={() => setDetails(true)}>
          View error details
        </button>
      </div>
      {details && (
        <ModelErrorModal error={item.lastError} final={item.message} onClose={() => setDetails(false)} />
      )}
    </div>
  );
}
