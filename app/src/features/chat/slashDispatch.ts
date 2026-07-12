import {
  activeTheme,
  configureOxenKey,
  exportFinetuning,
  exportLoop,
  getLoop,
  importLoop,
  listLoops,
  loopsPath,
  pickExportPath,
  pickLoopExportPath,
  pickLoopImportPath,
  removeLoop,
  saveLoop,
  setThemeLocation,
  themeLocation,
  useTheme,
} from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { CompressionMode, LoopSpec } from "../../lib/types";
import { parseSlashCommand, SLASH_COMMANDS } from "./slashCommands";

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
        if (args === "clear") state.setQueue([]);
        else if (args.startsWith("add ")) state.send(args.slice(4).trim());
        else note(queue.length ? queue.map((q, i) => `${i + 1}. ${q.text}`).join("\n") : "The queue is empty.");
        break;
      }
      case "/loop":
        await dispatchLoop(args, note);
        break;
      case "/code-review":
        if (args === "steps") state.openSettings("code-review");
        else state.startCodeReview(args || undefined);
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
          await configureOxenKey(id, args);
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

async function dispatchLoop(args: string, note: (text: string) => void) {
  const [sub = "list", ...tail] = args.split(/\s+/).filter(Boolean);
  const payload = tail.join(" ");
  const state = useStore.getState();
  if (sub === "list" || sub === "ls") {
    const loops = await listLoops();
    note(loops.map((l) => `${l.name} (${l.builtin ? "built-in" : "custom"}) — ${l.description}\n  gate: ${l.verify}`).join("\n"));
  } else if (sub === "show") {
    if (!payload) return note("Usage: /loop show <name>");
    const loop = await getLoop(payload);
    note(`${loop.name}\n${loop.description}\nGoal: ${loop.goal}\nStop: ${loop.max_iterations} iterations`);
  } else if (sub === "run" || sub === "go") {
    state.startLoop(payload || "default");
  } else if (sub === "goal") {
    if (payload) state.startLoop(undefined, payload);
    else note("Usage: /loop goal <what should be true when done>");
  } else if (sub === "new" || sub === "create") {
    const spec = interviewLoop();
    if (spec) {
      await saveLoop(spec);
      note(`Saved loop “${spec.name}”.`);
    }
  } else if (sub === "import") {
    const path = payload || (await pickLoopImportPath());
    if (path) note(`Imported loop “${(await importLoop(path)).name}”.`);
  } else if (sub === "export") {
    const [name, ...pathParts] = tail;
    if (!name) return note("Usage: /loop export <name> [path]");
    const path = pathParts.join(" ") || (await pickLoopExportPath(name));
    if (path) note(`Exported loop to ${await exportLoop(name, path)}.`);
  } else if (sub === "rm" || sub === "remove") {
    if (!payload) return note("Usage: /loop rm <name>");
    await removeLoop(payload);
    note(`Removed loop “${payload}”.`);
  } else if (sub === "path") note(await loopsPath());
  else note("Loop: list | run [name] | goal <text> | new | show <name> | import [path] | export <name> [path] | rm <name> | path");
}

function interviewLoop(): LoopSpec | null {
  const name = window.prompt("Loop name", "my-loop");
  if (name === null) return null;
  const goal = window.prompt("What should be true when the loop is done?");
  if (goal === null || !goal.trim()) return null;
  const command = window.prompt("Verification command (leave blank for a model-scored rubric)", "cargo test");
  const criteria = window.prompt("Success criteria, comma-separated (optional)", "") ?? "";
  const max = Number(window.prompt("Maximum iterations", "8")) || 8;
  return {
    schema_version: 2,
    name: name.trim() || "my-loop",
    description: `Created in the desktop app for: ${goal.trim()}`,
    goal: goal.trim(),
    success_criteria: criteria.split(",").map((s) => s.trim()).filter(Boolean),
    verify: command?.trim()
      ? { type: "command", command: command.trim(), timeout_ms: 300_000 }
      : { type: "rubric", threshold: 8 },
    gates: [],
    max_iterations: max,
    token_budget: null,
  };
}
