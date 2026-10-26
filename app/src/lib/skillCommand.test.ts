import { describe, expect, it } from "vitest";

import { splitSkillBlock, withSkillInstructions } from "./skillCommand";
import type { SkillInfo } from "./types";

const skill = (over: Partial<SkillInfo> = {}): SkillInfo => ({
  name: "deploy",
  description: "Deploys the app.",
  instructions: "1. Run `make deploy`.\n2. Report the URL.",
  scope: "project",
  dir: "/repo/.oxen-harness/skills/deploy",
  enabled: true,
  draft: false,
  ...over,
});

describe("withSkillInstructions", () => {
  it("bakes the skill's instructions after the user's request", () => {
    const out = withSkillInstructions("/deploy to staging", [skill()]);
    expect(out.startsWith("/deploy to staging\n\n<skill ")).toBe(true);
    expect(out).toContain("1. Run `make deploy`.");
    expect(out).toContain("/repo/.oxen-harness/skills/deploy");
  });

  it("round-trips through splitSkillBlock", () => {
    const out = withSkillInstructions("/deploy", [skill({ dir: '/odd "dir" & co' })]);
    const { text, skill: block } = splitSkillBlock(out);
    expect(text).toBe("/deploy");
    expect(block?.name).toBe("deploy");
    expect(block?.dir).toBe('/odd "dir" & co');
    expect(block?.instructions).toContain("2. Report the URL.");
  });

  it("leaves everything else alone", () => {
    const skills = [skill(), skill({ name: "off", enabled: false }), skill({ name: "wip", draft: true })];
    for (const text of ["deploy it", "/model", "/off now", "/wip", "/deployment", " /deploy"]) {
      expect(withSkillInstructions(text, skills)).toBe(text);
    }
  });

  it("never expands a message twice (a queued prompt is sent again later)", () => {
    const once = withSkillInstructions("/deploy", [skill()]);
    expect(withSkillInstructions(once, [skill()])).toBe(once);
  });
});

describe("splitSkillBlock", () => {
  it("ignores text that merely mentions a skill tag", () => {
    const text = 'Explain <skill name="x" dir="d"> tags';
    expect(splitSkillBlock(text)).toEqual({ text, skill: null });
  });
});
