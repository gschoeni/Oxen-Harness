// A compact, always-visible usage readout for the active chat, shown just above
// the composer. The empty-state hero carries the all-time "Total tokens used"
// stat; this fills the gap during a conversation, where the hero is gone —
// surfacing this session's live token count and how full the context window is.

import { useEffect, useState } from "react";
import { compactTokens, formatUsd } from "../../lib/format";
import { sessionCost } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { CompressionMode } from "../../lib/types";

const MODE_HINT: Record<CompressionMode, string> = {
  off: "Compression is off: requests are sent exactly as recorded.",
  audit:
    "Compression audit is armed: measuring what compression would save, without changing requests.",
  on: "Compression is on: stale tool output is compressed before each request (originals stay retrievable).",
};

export function TokenMeter() {
  const baseUsed = useStore((s) => s.session?.tokens_used ?? 0);
  const baseContext = useStore((s) => s.session?.context_tokens ?? 0);
  const contextWindow = useStore((s) => s.session?.context_window ?? 0);
  // Tokens streamed so far in the in-flight turn (reset to 0 at turn end), so the
  // meter climbs live instead of jumping only when the message finishes.
  const live = useStore((s) => Math.floor(s.session ? s.liveTokens[s.session.session_id] ?? 0 : 0));
  // Generation speed for the active/last turn in this session (tokens/sec).
  const tps = useStore((s) => (s.session ? s.tokensPerSecond[s.session.session_id] ?? 0 : 0));
  // What context compression saved this session (audit mode: would have saved).
  const saved = useStore((s) => (s.session ? s.compression[s.session.session_id] : undefined));
  // The live agent's actual mode — shown even before any savings exist, so
  // "armed but nothing eligible yet" is visible. The control for changing it
  // lives next to the model name in the composer, where it's reachable before
  // the first message too.
  const mode = useStore((s) => s.session?.compression_mode ?? "off");
  const model = useStore((s) => s.session?.model ?? "");
  const sessionId = useStore((s) => s.session?.session_id ?? "");
  const pricedUsage = useStore((s) => (sessionId ? s.sessionUsage[sessionId] : undefined));
  const [cost, setCost] = useState<number | null>(null);

  useEffect(() => {
    let active = true;
    if (!model || !pricedUsage) { setCost(null); return; }
    sessionCost(model, pricedUsage.prompt, pricedUsage.completion)
      .then((next) => active && setCost(next))
      .catch(() => active && setCost(null));
    return () => { active = false; };
  }, [model, pricedUsage?.prompt, pricedUsage?.completion]);

  const tokensUsed = baseUsed + live;
  const contextTokens = baseContext + live;
  const pct = contextWindow > 0 ? Math.min(100, (contextTokens / contextWindow) * 100) : 0;

  return (
    <div className="token-meter" title={`${contextTokens.toLocaleString()} / ${contextWindow.toLocaleString()} context tokens`}>
      <span className="token-meter-used">{tokensUsed.toLocaleString()} tokens used</span>
      {cost !== null && (
        <>
          <span className="token-meter-dot">·</span>
          <span className="token-meter-cost" title="Estimated cost for this session">{formatUsd(cost)}</span>
        </>
      )}
      {contextWindow > 0 && (
        <>
          <span className="token-meter-dot">·</span>
          <span className="token-meter-ctx">{pct < 1 ? "<1" : Math.round(pct)}% of context</span>
        </>
      )}
      {tps > 0 && (
        <>
          <span className="token-meter-dot">·</span>
          <span className="token-meter-tps" title="Generation speed">
            {tps >= 10 ? Math.round(tps) : tps.toFixed(1)} tok/s
          </span>
        </>
      )}
      {mode !== "off" && (
        <>
          <span className="token-meter-dot">·</span>
          <span className="token-meter-saved" title={MODE_HINT[mode]}>
            {mode === "audit" ? "would save" : "saved"} ~{compactTokens(saved?.tokensSaved ?? 0)}
          </span>
        </>
      )}
    </div>
  );
}
