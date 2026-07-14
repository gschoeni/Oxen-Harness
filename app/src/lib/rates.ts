import type { ModelPricing, OxenModelHit } from "./types";

/** A compact per-million-token price label, e.g. `$3/M in · $15/M out` —
 *  per-token rates are tiny fractions of a cent; scaling to a million tokens
 *  gives a number a human can compare (mirrors the CLI's format_rate). */
export function formatRate(pricing: ModelPricing | null): string | null {
  if (!pricing) return null;
  const perMillion = (perToken: number): string | null => {
    const m = perToken * 1_000_000;
    if (m <= 0) return null;
    return Number.isInteger(+m.toFixed(4)) ? `$${Math.round(m)}/M` : `$${m.toFixed(2)}/M`;
  };
  const input = perMillion(pricing.input_cost_per_token);
  const output = perMillion(pricing.output_cost_per_token);
  if (input && output) return `${input} in · ${output} out`;
  return input ? `${input} in` : output ? `${output} out` : null;
}

/** The formatted rate for every priced model in a catalog listing, keyed by
 *  model id — what pickers join against their own model lists. */
export function ratesById(hits: OxenModelHit[]): Map<string, string> {
  const rates = new Map<string, string>();
  for (const h of hits) {
    const rate = formatRate(h.pricing);
    if (rate) rates.set(h.id, rate);
  }
  return rates;
}
