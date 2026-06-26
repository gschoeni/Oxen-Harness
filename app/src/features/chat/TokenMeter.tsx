// A compact, always-visible usage readout for the active chat, shown just above
// the composer. The empty-state hero carries the all-time "Total tokens used"
// stat; this fills the gap during a conversation, where the hero is gone —
// surfacing this session's live token count and how full the context window is.

import { useStore } from "../../lib/store";

export function TokenMeter() {
  const baseUsed = useStore((s) => s.session?.tokens_used ?? 0);
  const baseContext = useStore((s) => s.session?.context_tokens ?? 0);
  const contextWindow = useStore((s) => s.session?.context_window ?? 0);
  // Tokens streamed so far in the in-flight turn (reset to 0 at turn end), so the
  // meter climbs live instead of jumping only when the message finishes.
  const live = useStore((s) => Math.floor(s.session ? s.liveTokens[s.session.session_id] ?? 0 : 0));

  const tokensUsed = baseUsed + live;
  const contextTokens = baseContext + live;
  const pct = contextWindow > 0 ? Math.min(100, (contextTokens / contextWindow) * 100) : 0;

  return (
    <div className="token-meter" title={`${contextTokens.toLocaleString()} / ${contextWindow.toLocaleString()} context tokens`}>
      <span className="token-meter-used">{tokensUsed.toLocaleString()} tokens used</span>
      {contextWindow > 0 && (
        <>
          <span className="token-meter-dot">·</span>
          <span className="token-meter-ctx">{pct < 1 ? "<1" : Math.round(pct)}% of context</span>
        </>
      )}
    </div>
  );
}
