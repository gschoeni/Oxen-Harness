import { describe, expect, it } from "vitest";

// Every stylesheet in the app, keyed by path, so a width that should come
// from a token can be caught wherever it is written.
const sheets = import.meta.glob("/src/**/*.css", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;
const isTokens = (path: string) => path.endsWith("/styles/tokens.css");
// pixel.css is the theme skin layer: it *defines* framing values, like tokens.css.
const definesTokens = (path: string) => isTokens(path) || path.endsWith("/styles/pixel.css");

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

// Properties whose values come from a token scale: type (--text-*), space
// (--space-*), and radius (--radius-*). A literal px here is a size the scale
// doesn't know about, so it drifts from every neighbour that used the scale.
const SCALED = /^(font-size|line-height|gap|row-gap|column-gap|padding(-top|-right|-bottom|-left|-inline|-block)?|margin(-top|-right|-bottom|-left|-inline|-block)?|border-radius)$/;
const DECL = /([a-z-]+)\s*:\s*([^;{}]+);/g;
const PX = /(?<![\w.-])-?\d*\.?\d+px\b/;

function literalSizes(css: string): string[] {
  const hits: string[] = [];
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const m of stripped.matchAll(DECL)) {
    const [, prop, value] = m;
    if (SCALED.test(prop) && PX.test(value)) hits.push(`${prop}: ${value.trim()}`);
  }
  return hits;
}

// The ratchet. Each entry is how many literal sizes a stylesheet still has on
// scaled properties. Migrate a file to tokens and lower its number (or drop
// it); never raise one, and a new stylesheet starts at zero. The migration is
// tracked in 04-backlog.md.
const BASELINE: Record<string, number> = {
  "/src/app.css": 1,
  "/src/components/ui/markdown.css": 1,
  "/src/components/ui/ui.css": 5,
  "/src/features/approvals/approvals.css": 3,
  "/src/features/canvas/canvas.css": 4,
  "/src/features/chat/agents.css": 41,
  "/src/features/chat/agentview.css": 3,
  "/src/features/chat/apikey.css": 2,
  "/src/features/chat/chat.css": 43,
  "/src/features/chat/gamedock.css": 5,
  "/src/features/chat/plan.css": 2,
  "/src/features/chat/toolcall.css": 6,
  "/src/features/docks/docks.css": 7,
  "/src/features/files/files.css": 40,
  "/src/features/history/history.css": 7,
  "/src/features/inspector/dev.css": 6,
  "/src/features/logs/logs.css": 4,
  "/src/features/media/media.css": 5,
  "/src/features/models/models.css": 4,
  "/src/features/preview/preview.css": 6,
  "/src/features/projects/projects.css": 2,
  "/src/features/questions/questions.css": 4,
  "/src/features/rules/rules.css": 18,
  "/src/features/settings/permissions.css": 2,
  "/src/features/settings/settings.css": 46,
  "/src/features/settings/teaching.css": 2,
  "/src/features/skills/skills.css": 5,
  "/src/features/tabs/tabs.css": 6,
  "/src/features/tools/tools.css": 3,
};

// The scale itself: every step has to be legible on a laptop screen without
// leaning in. 10px labels crept in through the smallest token, so the floor
// is pinned here alongside the body size everything else is set against.
describe("type scale", () => {
  const tokens = Object.entries(sheets).find(([path]) => isTokens(path))?.[1] ?? "";
  const scale = new Map(
    [...tokens.matchAll(/--text-(\w+):\s*([\d.]+)px;/g)].map((m) => [m[1], Number(m[2])] as const),
  );

  it("defines every step in tokens.css", () => {
    expect([...scale.keys()]).toEqual(["3xs", "2xs", "xs", "sm", "md", "lg", "xl", "2xl"]);
  });

  it("keeps the smallest step readable and body text at 15px", () => {
    for (const [name, px] of scale) expect(px, `--text-${name}`).toBeGreaterThanOrEqual(11);
    expect(scale.get("md")).toBe(15);
  });

  it("grows monotonically so a larger name is always a larger size", () => {
    const sizes = [...scale.values()];
    for (let i = 1; i < sizes.length; i++) expect(sizes[i]).toBeGreaterThan(sizes[i - 1]);
  });
});

describe("sizing ratchet", () => {
  it("adds no new literal sizes on properties that have a token scale", () => {
    const report: string[] = [];
    for (const [path, css] of Object.entries(sheets)) {
      if (definesTokens(path)) continue;
      const hits = literalSizes(css);
      const allowed = BASELINE[path] ?? 0;
      if (hits.length > allowed) {
        report.push(
          `${path}: ${hits.length} literal sizes, baseline ${allowed}. Use a token, or if the ` +
            `size is deliberate set "${path}": ${hits.length}\n    ${hits.join("\n    ")}`,
        );
      }
    }
    expect(report, report.join("\n\n")).toEqual([]);
  });

  it("keeps the baseline honest: no entry above the real count", () => {
    const stale = Object.entries(BASELINE)
      .map(([path, allowed]) => [path, allowed, literalSizes(sheets[path] ?? "").length] as const)
      .filter(([, allowed, actual]) => allowed > actual)
      .map(([path, allowed, actual]) =>
        actual === 0 && !(path in sheets)
          ? `${path} is gone: delete its entry`
          : `${path}: baseline ${allowed} > actual ${actual}: set "${path}": ${actual}`,
      );
    expect(stale, stale.join("\n")).toEqual([]);
  });
});
