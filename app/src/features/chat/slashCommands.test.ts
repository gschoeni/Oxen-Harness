import { describe, expect, it } from "vitest";
import { parseSlashCommand, slashSuggestions, SLASH_COMMANDS } from "./slashCommands";

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
});
