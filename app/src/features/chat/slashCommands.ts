import { invocableSkills } from "../../lib/skillCommand";
import type { SkillInfo } from "../../lib/types";

export interface SlashCommand {
  name: string;
  aliases?: string[];
  description: string;
  usage?: string;
  /** Set when this command runs a skill (`/<skill-name> [request]`). */
  skill?: string;
}

export const SLASH_COMMANDS: SlashCommand[] = [
  { name: "/model", description: "Pick or switch the model", usage: "/model [id]" },
  { name: "/theme", aliases: ["/themes"], description: "Change the theme", usage: "/theme [use <name>]" },
  { name: "/queue", description: "Show or manage queued messages", usage: "/queue [add|clear]" },
  { name: "/code-review", aliases: ["/review"], description: "Review uncommitted changes or compare a branch", usage: "/code-review [base-branch]" },
  { name: "/export", description: "Export this transcript as JSONL", usage: "/export [path]" },
  { name: "/skills", aliases: ["/skill"], description: "Open the discovered skills" },
  { name: "/retry", aliases: ["/continue"], description: "Retry an interrupted turn" },
  { name: "/location", aliases: ["/departing"], description: "Show or set the themed location", usage: "/location [place|clear]" },
  { name: "/auth", aliases: ["/login"], description: "Configure the Oxen API key", usage: "/auth [key]" },
  { name: "/compression", aliases: ["/compress"], description: "Show or switch context compression", usage: "/compression [off|audit|on]" },
  { name: "/usage", description: "Open token usage and estimated spend" },
  { name: "/repository", aliases: ["/repo"], description: "See or switch the Oxen repository this project's assets go to", usage: "/repository [namespace/name|clear]" },
  { name: "/help", aliases: ["/?"], description: "Show available slash commands" },
];

export interface ParsedSlashCommand {
  command: SlashCommand;
  invokedAs: string;
  args: string;
}

/** Every command on offer: the built-ins, then each runnable skill. A
 *  built-in keeps its name (and aliases) when a skill shares it. */
export function allCommands(skills: SkillInfo[] = []): SlashCommand[] {
  const taken = new Set(SLASH_COMMANDS.flatMap((c) => [c.name, ...(c.aliases ?? [])]));
  const fromSkills = invocableSkills(skills)
    .map((s) => ({
      name: `/${s.name}`,
      description: s.description,
      usage: `/${s.name} [request]`,
      skill: s.name,
    }))
    .filter((c) => !taken.has(c.name));
  return [...SLASH_COMMANDS, ...fromSkills];
}

export function parseSlashCommand(text: string, skills: SkillInfo[] = []): ParsedSlashCommand | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("/")) return null;
  const split = trimmed.search(/\s/);
  const invokedAs = split < 0 ? trimmed : trimmed.slice(0, split);
  const command = allCommands(skills).find(
    (entry) => entry.name === invokedAs || entry.aliases?.includes(invokedAs),
  );
  if (!command) return null;
  return { command, invokedAs, args: split < 0 ? "" : trimmed.slice(split).trim() };
}

export function slashSuggestions(text: string, skills: SkillInfo[] = []): SlashCommand[] {
  if (!text.startsWith("/") || /\s/.test(text)) return [];
  const query = text.toLowerCase();
  return allCommands(skills).filter(
    (entry) => entry.name.startsWith(query) || entry.aliases?.some((alias) => alias.startsWith(query)),
  );
}
