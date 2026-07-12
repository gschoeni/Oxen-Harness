export type QueueCommandResult =
  | { kind: "show"; items: string[] }
  | { kind: "update"; items: string[]; message: string }
  | { kind: "run"; items: string[] }
  | { kind: "error"; message: string };

/** Apply the CLI's 1-based `/queue` operations without touching UI state. */
export function queueCommand(args: string, items: string[]): QueueCommandResult {
  const [sub = "list", ...tail] = args.split(/\s+/).filter(Boolean);
  const payload = tail.join(" ");
  if (sub === "list" || sub === "ls") return { kind: "show", items };
  if (sub === "add" || sub === "push") {
    if (!payload) return { kind: "error", message: "Usage: /queue add <message>" };
    return { kind: "update", items: [...items, payload], message: `Queued message #${items.length + 1}.` };
  }
  if (sub === "clear") return { kind: "update", items: [], message: "Queue cleared." };
  if (sub === "run" || sub === "go") return items.length ? { kind: "run", items } : { kind: "error", message: "The queue is empty." };

  const position = Number(tail[0]);
  const index = position - 1;
  if (!Number.isInteger(position) || index < 0 || index >= items.length) {
    return { kind: "error", message: `Queue position must be between 1 and ${items.length}.` };
  }
  const next = [...items];
  if (sub === "edit") {
    const text = tail.slice(1).join(" ");
    if (!text) return { kind: "error", message: "Usage: /queue edit <n> <new message>" };
    next[index] = text;
  } else if (sub === "rm" || sub === "remove" || sub === "del") next.splice(index, 1);
  else if (sub === "up" || sub === "down") {
    const target = index + (sub === "up" ? -1 : 1);
    if (target < 0 || target >= next.length) return { kind: "error", message: `Message #${position} cannot move ${sub}.` };
    [next[index], next[target]] = [next[target], next[index]];
  } else {
    return { kind: "error", message: "Queue: list | add <msg> | edit <n> <msg> | up <n> | down <n> | rm <n> | clear | run" };
  }
  return { kind: "update", items: next, message: "Queue updated." };
}
