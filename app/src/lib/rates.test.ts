import { describe, expect, it } from "vitest";
import { formatRate, ratePartsById, rateParts } from "./rates";
import type { OxenModelHit } from "./types";

const hit = (id: string, pricing: OxenModelHit["pricing"]): OxenModelHit =>
  ({ id, pricing }) as OxenModelHit;

describe("rateParts", () => {
  it("scales per-token prices to whole dollars per million, keeping cents only when needed", () => {
    expect(rateParts({ input_cost_per_token: 3e-6, output_cost_per_token: 1.5e-5 })).toEqual({
      input: "$3",
      output: "$15",
    });
    expect(rateParts({ input_cost_per_token: 2.5e-7, output_cost_per_token: 1e-6 })).toEqual({
      input: "$0.25",
      output: "$1",
    });
  });

  it("drops a zero-priced side and is null when nothing is priced", () => {
    expect(rateParts({ input_cost_per_token: 0, output_cost_per_token: 2e-6 })).toEqual({
      input: null,
      output: "$2",
    });
    expect(rateParts({ input_cost_per_token: 0, output_cost_per_token: 0 })).toBeNull();
    expect(rateParts(null)).toBeNull();
  });
});

describe("formatRate", () => {
  it("composes the one-line label the settings page and CLI share", () => {
    expect(formatRate({ input_cost_per_token: 3e-6, output_cost_per_token: 1.5e-5 })).toBe(
      "$3/M in · $15/M out",
    );
    expect(formatRate({ input_cost_per_token: 0, output_cost_per_token: 6e-6 })).toBe("$6/M out");
    expect(formatRate(null)).toBeNull();
  });
});

describe("ratePartsById", () => {
  it("indexes only the priced models", () => {
    const rates = ratePartsById([
      hit("a", { input_cost_per_token: 1e-6, output_cost_per_token: 2e-6 }),
      hit("b", null),
    ]);
    expect([...rates.keys()]).toEqual(["a"]);
    expect(rates.get("a")).toEqual({ input: "$1", output: "$2" });
  });
});
