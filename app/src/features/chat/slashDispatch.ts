import {
  activeTheme,
  exportFinetuning,
  pickExportPath,
  setThemeLocation,
  themeLocation,
  useTheme,
} from "../../lib/ipc";
import { advancedSettingsEnabled } from "../../lib/features";
import { useStore } from "../../lib/store";
import type { CompressionMode } from "../../lib/types";
import { parseSlashCommand, SLASH_COMMANDS } from "./slashCommands";
import { queueCommand } from "./queueCommand";

const help = () => SLASH_COMMANDS.map((c) => `${c.usage ?? c.name} — ${c.description}`).join("\n");

/** Execute recognized desktop commands locally. False means the text is an
 * unknown slash-prefixed prompt and should still be sent to the model. */
export async function dispatchSlashCommand(text: string): Promise<boolean> {
  const parsed = parseSlashCommand(text);
  if (!parsed) return false;
  const state = useStore.getState();
  const note = state.addNotice;
  const args = parsed.args;

  try {
    switch (parsed.command.name) {
      case "/help":
        note(help());
        break;
      case "/model":
        if (args) await state.changeModel(args);
        else state.openSettings("cloud-models");
        break;
      case "/theme": {
        const name = args.replace(/^use\s+/, "").trim();
        if (!name) state.openSettings("appearance");
        else state.applyTheme(await useTheme(name));
        break;
      }
      case "/queue": {
        const queue = state.session ? state.queues[state.session.session_id] ?? [] : [];
        const result = queueCommand(args, queue.map((q) => q.text));
        if (result.kind === "show")
          note(result.items.length ? result.items.map((item, i) => `${i + 1}. ${item}`).join("\n") : "The queue is empty.");
        else if (result.kind === "update") {
          state.setQueue(result.items);
          note(result.message);
        } else if (result.kind === "run") {
          const [first, ...rest] = result.items;
          state.setQueue(rest);
          state.send(first);
        } else note(result.message);
        break;
      }
      case "/code-review":
        if (args === "steps") {
          if (advancedSettingsEnabled()) state.openSettings("code-review");
          else note("The review steps can't be edited in this release.");
        } else state.startCodeReview(args || undefined);
        break;
      case "/export": {
        const id = state.session?.session_id;
        if (!id) break;
        const path = args || (await pickExportPath(`${id}.jsonl`));
        if (path) note(`Exported ${await exportFinetuning(path, [id], true)} conversation to ${path}.`);
        break;
      }
      case "/skills":
        state.openSettings("skills");
        break;
      case "/retry": {
        const id = state.session?.session_id;
        const item = id ? [...(state.threads[id] ?? [])].reverse().find((i) => i.kind === "retry") : undefined;
        if (id && item?.kind === "retry") state.retryBrokenTurn(id, item.id);
        else note("Nothing to retry — the last turn finished.");
        break;
      }
      case "/location": {
        if (!args) note((await themeLocation()) ?? "No custom location is set.");
        else {
          await setThemeLocation(args === "clear" ? null : args);
          state.applyTheme(await activeTheme());
          note(args === "clear" ? "Location reset to the active theme." : `Location set to ${args}.`);
        }
        break;
      }
      case "/auth":
        if (!args) state.openSettings("connection");
        else {
          const id = state.session?.session_id;
          if (!id) break;
          await state.saveApiKey(id, args);
          note("Oxen API key saved.");
        }
        break;
      case "/compression":
        if (!args) state.openSettings("compression");
        else if (["off", "audit", "on"].includes(args)) await state.changeCompressionMode(args as CompressionMode);
        else note("Usage: /compression off|audit|on");
        break;
      case "/usage":
        state.openSettings("usage");
        break;
    }
  } catch (error) {
    note(`${parsed.command.name} failed: ${String(error)}`);
  }
  return true;
}
