export interface SlashCommand {
  name: string;
  aliases?: string[];
  description: string;
  usage?: string;
}

export const SLASH_COMMANDS: SlashCommand[] = [
  { name: "/model", description: "Pick or switch the model", usage: "/model [id]" },
  { name: "/theme", aliases: ["/themes"], description: "Change the theme", usage: "/theme [use <name>]" },
  { name: "/queue", description: "Show or manage queued messages", usage: "/queue [add|clear]" },
  { name: "/loop", aliases: ["/loops"], description: "Run or manage verification loops", usage: "/loop [list|show|new|run|goal|import|export|rm]" },
  { name: "/code-review", aliases: ["/review"], description: "Review uncommitted changes or compare a branch", usage: "/code-review [base-branch]" },
  { name: "/export", description: "Export this transcript as JSONL", usage: "/export [path]" },
  { name: "/skills", aliases: ["/skill"], description: "Open the discovered skills" },
  { name: "/retry", aliases: ["/continue"], description: "Retry an interrupted turn" },
  { name: "/location", aliases: ["/departing"], description: "Show or set the themed location", usage: "/location [place|clear]" },
  { name: "/auth", aliases: ["/login"], description: "Configure the Oxen API key", usage: "/auth [key]" },
  { name: "/compression", aliases: ["/compress"], description: "Show or switch context compression", usage: "/compression [off|audit|on]" },
  { name: "/usage", description: "Open token usage and estimated spend" },
  { name: "/help", aliases: ["/?"], description: "Show available slash commands" },
];

export interface ParsedSlashCommand {
  command: SlashCommand;
  invokedAs: string;
  args: string;
}

export function parseSlashCommand(text: string): ParsedSlashCommand | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("/")) return null;
  const split = trimmed.search(/\s/);
  const invokedAs = split < 0 ? trimmed : trimmed.slice(0, split);
  const command = SLASH_COMMANDS.find(
    (entry) => entry.name === invokedAs || entry.aliases?.includes(invokedAs),
  );
  if (!command) return null;
  return { command, invokedAs, args: split < 0 ? "" : trimmed.slice(split).trim() };
}

export function slashSuggestions(text: string): SlashCommand[] {
  if (!text.startsWith("/") || /\s/.test(text)) return [];
  const query = text.toLowerCase();
  return SLASH_COMMANDS.filter(
    (entry) => entry.name.startsWith(query) || entry.aliases?.some((alias) => alias.startsWith(query)),
  );
}
