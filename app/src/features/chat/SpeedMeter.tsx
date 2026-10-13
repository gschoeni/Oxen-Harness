// The live generation-speed readout in the usage meter row: how fast the
// model is writing right now (rate + a sparkline of the moment), settling on
// the call's exact rate and time-to-first-token once it finishes. Fed by
// `agent://throughput` through `lib/speed.ts`.

import { Zap } from "lucide-react";
import { formatRate, SPARK_SAMPLES, type SpeedState } from "../../lib/speed";
import { useStore } from "../../lib/store";

const BAR_W = 2;
const BAR_GAP = 1;
const SPARK_H = 12;

export function SpeedMeter() {
  const speed = useStore((s) => (s.session ? s.speed[s.session.session_id] : undefined));
  const running = useStore((s) => (s.session ? s.runStatus[s.session.session_id] === "running" : false));
  if (!speed || speed.tokensPerSec === null) return null;
  // Live only while this chat's turn runs: a turn that ended without a final
  // reading (a stop) must not leave a sparkline twitching on an idle chat.
  const live = running && !speed.done;
  return (
    <span className="speed-meter" data-live={live || undefined} title={tooltip(speed)} aria-label={`${formatRate(speed.tokensPerSec)} tokens per second`}>
      <Zap className="speed-meter-bolt" size={12} strokeWidth={2.25} aria-hidden />
      <span className="speed-meter-rate">{formatRate(speed.tokensPerSec)}</span>
      <span className="speed-meter-unit">tok/s</span>
      {live ? <Sparkline samples={speed.samples} /> : <span className="speed-meter-ttft">{(speed.ttftMs / 1000).toFixed(1)}s first token</span>}
    </span>
  );
}

/** Bars scaled against their own peak, so the shape reads the same for a
 *  9 tok/s laptop model and a 200 tok/s cloud one. A fixed-width track: the
 *  newest sample is always the rightmost bar, and slots not yet filled show
 *  as a faint baseline, so the row never shifts as the window fills. */
function Sparkline({ samples }: { samples: number[] }) {
  const peak = Math.max(0, ...samples);
  const width = SPARK_SAMPLES * (BAR_W + BAR_GAP) - BAR_GAP;
  const empty = SPARK_SAMPLES - samples.length;
  return (
    <svg className="speed-meter-spark" width={width} height={SPARK_H} viewBox={`0 0 ${width} ${SPARK_H}`} aria-hidden>
      {Array.from({ length: SPARK_SAMPLES }, (_, slot) => {
        const i = slot - empty;
        const v = i >= 0 ? samples[i] : null;
        const h = v === null ? 1 : peak > 0 ? Math.max(1, Math.round((v / peak) * SPARK_H)) : 1;
        const cls = v === null ? "speed-meter-bar-empty" : i === samples.length - 1 ? "speed-meter-bar-now" : undefined;
        return <rect key={slot} x={slot * (BAR_W + BAR_GAP)} y={SPARK_H - h} width={BAR_W} height={h} rx={0.5} className={cls} />;
      })}
    </svg>
  );
}

function tooltip(s: SpeedState): string {
  const parts = [
    s.model,
    `${s.tokensPerSec === null ? "—" : formatRate(s.tokensPerSec)} tokens/sec`,
    `${(s.ttftMs / 1000).toFixed(1)}s to first token`,
  ];
  if (s.done) parts.push(`${s.outputTokens.toLocaleString()} tokens in ${(s.decodeMs / 1000).toFixed(1)}s`);
  return parts.join(" · ");
}
