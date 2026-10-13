// The live tokens/sec readout's state, folded from `agent://throughput`.
//
// The backend reports cumulative readings per model call (estimated while it
// streams, exact on the call's final `done` reading — reasoning and tool-call
// arguments included, which visible text alone would miss). The headline is
// the call's decode rate; `samples` holds the *instantaneous* rate between
// consecutive readings for the sparkline, so a stall or a burst shows instead
// of being averaged away. Mirrors the CLI's `speed.rs`.

import type { ThroughputEvent } from "./types";

/** How many instantaneous samples the sparkline spans (~4s of streaming). */
export const SPARK_SAMPLES = 16;

export interface SpeedState {
  model: string;
  /** Decode rate of the call in flight, or of the last finished one. */
  tokensPerSec: number | null;
  /** Wait from request to first generated token, in ms. */
  ttftMs: number;
  outputTokens: number;
  /** The call finished: the figures are settled. */
  done: boolean;
  /** Instantaneous tokens/sec, oldest first. */
  samples: number[];
  /** The latest reading's decode clock, for the next sample's delta. */
  decodeMs: number;
}

/** Fold one reading into the session's speed state. */
export function nextSpeed(prev: SpeedState | undefined, e: ThroughputEvent): SpeedState {
  let samples = prev?.samples ?? [];
  // Only live (estimated) readings of one call difference cleanly; the final
  // reading swaps in the endpoint's exact count.
  if (prev && !prev.done && !e.done && prev.model === e.model) {
    const dt = e.decode_ms - prev.decodeMs;
    const dTokens = e.output_tokens - prev.outputTokens;
    if (dt > 0 && dTokens >= 0) {
      samples = [...samples, (dTokens * 1000) / dt].slice(-SPARK_SAMPLES);
    }
  }
  // A reading with no rate — a new call's first tokens, or a whole call too
  // short to time (a quick tool call, a one-line answer) — never replaces a
  // timed one: the readout would otherwise blank at the end of most agentic
  // turns. Mirrors the CLI's `SpeedGauge::record`.
  if (e.tokens_per_sec === null && prev && prev.tokensPerSec !== null) return prev;
  return {
    model: e.model,
    tokensPerSec: e.tokens_per_sec,
    ttftMs: e.ttft_ms,
    outputTokens: e.output_tokens,
    done: e.done,
    samples,
    decodeMs: e.decode_ms,
  };
}

/** A rate at the precision a glance needs. */
export function formatRate(rate: number): string {
  return rate < 100 ? rate.toFixed(1) : Math.round(rate).toString();
}
