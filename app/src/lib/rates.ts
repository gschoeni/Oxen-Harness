import type { ModelPricing, OxenModelHit } from "./types";

/** A model's per-million-token prices as bare dollar figures (`$3`, `$0.50`),
 *  one side per direction; a side is null when the catalog prices it at zero. */
export interface RateParts {
  input: string | null;
  output: string | null;
}

/** Per-token rates are tiny fractions of a cent; scaling to a million tokens
 *  gives a number a human can compare (mirrors the CLI's format_rate). Null
 *  when neither direction is priced, so callers can skip the tag entirely. */
export function rateParts(pricing: ModelPricing | null): RateParts | null {
  if (!pricing) return null;
  const perMillion = (perToken: number): string | null => {
    const m = perToken * 1_000_000;
    if (m <= 0) return null;
    // Whole-dollar rates read cleaner without trailing zeros ($3), fractional
    // ones keep two decimals ($0.50).
    return Number.isInteger(+m.toFixed(4)) ? `$${Math.round(m)}` : `$${m.toFixed(2)}`;
  };
  const parts = {
    input: perMillion(pricing.input_cost_per_token),
    output: perMillion(pricing.output_cost_per_token),
  };
  return parts.input || parts.output ? parts : null;
}

/** A compact one-line price label, e.g. `$3/M in · $15/M out`. */
export function formatRate(pricing: ModelPricing | null): string | null {
  const parts = rateParts(pricing);
  if (!parts) return null;
  const { input, output } = parts;
  if (input && output) return `${input}/M in · ${output}/M out`;
  return input ? `${input}/M in` : `${output}/M out`;
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

/** Like {@link ratesById}, but keeping the figures apart for a layout that
 *  sets them in its own type rather than one mono string. */
export function ratePartsById(hits: OxenModelHit[]): Map<string, RateParts> {
  const rates = new Map<string, RateParts>();
  for (const h of hits) {
    const parts = rateParts(h.pricing);
    if (parts) rates.set(h.id, parts);
  }
  return rates;
}
