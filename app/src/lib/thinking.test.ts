import { describe, expect, it } from "vitest";

import {
  FRAMES_PER_PHRASE,
  elapsedLabel,
  glyphAt,
  phraseAt,
  spinnerGlyphs,
  thinkingPhrases,
  writingPhrases,
} from "./thinking";
import type { Theme } from "./types";

const theme = {
  voice: {
    spinner_glyphs: ["a", "b", "c"],
    thinking: ["Fording the river", "Yoking the oxen"],
    tool_verbs: { default: ["Working the trail"], write_file: ["Inscribing the ledger"] },
  },
} as unknown as Theme;

describe("thinking pools", () => {
  it("reads phrases and glyphs from the theme voice", () => {
    expect(thinkingPhrases(theme)).toEqual(["Fording the river", "Yoking the oxen"]);
    expect(spinnerGlyphs(theme)).toEqual(["a", "b", "c"]);
    expect(writingPhrases(theme)).toEqual(["Inscribing the ledger"]);
  });

  it("falls back when the theme is missing or sparse", () => {
    expect(thinkingPhrases(null)).toEqual(["Thinking"]);
    expect(spinnerGlyphs(null).length).toBeGreaterThan(0);
    // No write_file verbs → writing speaks in thinking phrases (CLI parity).
    const sparse = { voice: { thinking: ["Pondering"], tool_verbs: {} } } as unknown as Theme;
    expect(writingPhrases(sparse)).toEqual(["Pondering"]);
  });
});

describe("rotation rhythm", () => {
  it("rotates the phrase once per FRAMES_PER_PHRASE frames, wrapping", () => {
    const pool = ["one", "two", "three"];
    expect(phraseAt(pool, 1, 0)).toBe("two");
    expect(phraseAt(pool, 1, FRAMES_PER_PHRASE - 1)).toBe("two");
    expect(phraseAt(pool, 1, FRAMES_PER_PHRASE)).toBe("three");
    expect(phraseAt(pool, 1, 2 * FRAMES_PER_PHRASE)).toBe("one");
  });

  it("cycles glyphs every frame", () => {
    expect(glyphAt(["a", "b"], 0)).toBe("a");
    expect(glyphAt(["a", "b"], 1)).toBe("b");
    expect(glyphAt(["a", "b"], 2)).toBe("a");
    expect(glyphAt([], 5)).toBe("");
  });
});

describe("elapsedLabel", () => {
  it("formats like the CLI timer", () => {
    expect(elapsedLabel(7_000)).toBe("7s");
    expect(elapsedLabel(67_000)).toBe("1m07s");
    expect(elapsedLabel(-50)).toBe("0s");
  });
});
