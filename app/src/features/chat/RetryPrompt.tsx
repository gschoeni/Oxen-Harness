import { useEffect, useState } from "react";
import { CreditCard, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui";
import { getConnection } from "../../lib/ipc";
import { isCreditsError, useStore } from "../../lib/store";
import type { Item } from "./thread";
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
          <div className="apikey-sub">
            {item.message}
            {credits
              ? " Add credits to your account, then pick up right where you left off."
              : " Nothing is lost — retry as-is, or switch models or check your connection first, then pick up right where you left off."}
          </div>
        </div>
      </div>

      <div className="retry-actions">
        {credits && (
          <a className="retry-add-credits" href={`${hubUrl}/settings`} target="_blank" rel="noreferrer">
            Add credits
          </a>
        )}
        <Button
          variant="primary"
          size="sm"
          disabled={!sessionId || running}
          onClick={() => sessionId && retryBrokenTurn(sessionId, item.id)}
        >
          {credits ? "I’ve added credits — retry" : "Continue"}
          <RotateCcw size={15} />
        </Button>
      </div>

      {credits && (
        <div className="apikey-foot">
          Manage billing at <code>{host || "hub.oxen.ai"}/settings</code>. Retrying continues this
          conversation — nothing is lost.
        </div>
      )}
    </div>
  );
}
