import { describe, expect, it } from "vitest";
import { formatRate, nextSpeed, SPARK_SAMPLES } from "./speed";
import type { ThroughputEvent } from "./types";

const reading = (over: Partial<ThroughputEvent>): ThroughputEvent => ({
  session: "s",
  model: "m",
  output_tokens: 0,
  tokens_per_sec: null,
  decode_ms: 0,
  ttft_ms: 800,
  exact: false,
  done: false,
  ...over,
});

describe("nextSpeed", () => {
  it("plots the instantaneous rate between live readings", () => {
    let s = nextSpeed(undefined, reading({ output_tokens: 10, decode_ms: 500, tokens_per_sec: 20 }));
    s = nextSpeed(s, reading({ output_tokens: 30, decode_ms: 1000, tokens_per_sec: 30 }));
    s = nextSpeed(s, reading({ output_tokens: 40, decode_ms: 1500, tokens_per_sec: 26.7 }));
    expect(s.samples).toEqual([40, 20]);
    expect(s.tokensPerSec).toBe(26.7);
    expect(s.done).toBe(false);
  });

  it("settles on the exact final reading without plotting it", () => {
    let s = nextSpeed(undefined, reading({ output_tokens: 30, decode_ms: 1000, tokens_per_sec: 30 }));
    s = nextSpeed(s, reading({ output_tokens: 446, decode_ms: 10_000, tokens_per_sec: 44.6, exact: true, done: true }));
    expect(s).toMatchObject({ tokensPerSec: 44.6, done: true, outputTokens: 446, samples: [] });
  });

  it("keeps the last rate up while the next call waits for a rate", () => {
    let s = nextSpeed(undefined, reading({ output_tokens: 446, decode_ms: 10_000, tokens_per_sec: 44.6, done: true }));
    s = nextSpeed(s, reading({ output_tokens: 1, decode_ms: 0 }));
    expect(s.tokensPerSec).toBe(44.6);
    expect(s.done).toBe(false);
  });

  it("caps the sparkline window", () => {
    let s = nextSpeed(undefined, reading({ tokens_per_sec: 1 }));
    for (let i = 1; i <= SPARK_SAMPLES + 5; i++) {
      s = nextSpeed(s, reading({ output_tokens: i * 10, decode_ms: i * 250, tokens_per_sec: 40 }));
    }
    expect(s.samples).toHaveLength(SPARK_SAMPLES);
  });

  it("formats rates for a glance", () => {
    expect(formatRate(8.44)).toBe("8.4");
    expect(formatRate(212.4)).toBe("212");
  });
});
