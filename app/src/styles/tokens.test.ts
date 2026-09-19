import { describe, expect, it } from "vitest";

// Every stylesheet in the app, keyed by path, so a width that should come
// from a token can be caught wherever it is written.
const sheets = import.meta.glob("/src/**/*.css", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;
const isTokens = (path: string) => path.endsWith("/styles/tokens.css");

describe("chat column tokens", () => {
  it("defines the chat column once, in tokens.css", () => {
    const tokens = Object.entries(sheets).find(([path]) => isTokens(path))?.[1] ?? "";
    expect(tokens).toMatch(/--chat-column:\s*740px;/);
    expect(tokens).toMatch(/--chat-gutter:\s*var\(--space-5\);/);
  });

  it("never repeats the chat column width as a literal", () => {
    expect(Object.keys(sheets).length).toBeGreaterThan(5);
    const offenders = Object.entries(sheets)
      .filter(([path, css]) => !isTokens(path) && /\b740px\b/.test(css))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
