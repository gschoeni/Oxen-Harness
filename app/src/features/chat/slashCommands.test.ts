import { describe, expect, it } from "vitest";
import { parseSlashCommand, slashSuggestions, SLASH_COMMANDS } from "./slashCommands";
import type { SkillInfo } from "../../lib/types";

const skill = (name: string, over: Partial<SkillInfo> = {}): SkillInfo => ({
  name,
  description: `Does ${name}.`,
  instructions: "Steps.",
  scope: "project",
  dir: `/repo/.oxen-harness/skills/${name}`,
  enabled: true,
  draft: false,
  ...over,
});

describe("desktop slash commands", () => {
  it("offers every desktop command when slash is typed, without CLI exit commands", () => {
    expect(slashSuggestions("/")).toEqual(SLASH_COMMANDS);
    expect(SLASH_COMMANDS.map((c) => c.name)).not.toContain("/exit");
    expect(SLASH_COMMANDS.map((c) => c.name)).not.toContain("/quit");
  });

  it("filters canonical names and parses aliases with unsplit arguments", () => {
    expect(slashSuggestions("/co").map((c) => c.name)).toEqual([
      "/code-review",
      "/retry",
      "/compression",
    ]);
    expect(parseSlashCommand("  /review   origin/main  ")).toMatchObject({
      command: { name: "/code-review" },
      invokedAs: "/review",
      args: "origin/main",
    });
  });

  it("leaves unknown slash-prefixed prompts alone", () => {
    expect(parseSlashCommand("/frobnicate this")).toBeNull();
  });

  it("offers runnable skills after the built-ins, which keep their names", () => {
    const skills = [
      skill("deploy"),
      skill("model"), // clashes with a built-in
      skill("skill"), // clashes with a built-in alias
      skill("off", { enabled: false }),
      skill("wip", { draft: true }),
    ];
    const names = slashSuggestions("/", skills).map((c) => c.name);
    expect(names.slice(0, SLASH_COMMANDS.length)).toEqual(SLASH_COMMANDS.map((c) => c.name));
    expect(names.slice(SLASH_COMMANDS.length)).toEqual(["/deploy"]);
    expect(slashSuggestions("/depl", skills)).toMatchObject([
      { name: "/deploy", skill: "deploy", description: "Does deploy." },
    ]);
    expect(parseSlashCommand("/deploy to prod", skills)).toMatchObject({
      command: { skill: "deploy" },
      args: "to prod",
    });
    expect(parseSlashCommand("/model x", skills)?.command.skill).toBeUndefined();
  });
});
