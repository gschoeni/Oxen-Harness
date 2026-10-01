import type { OxenModelHit } from "./types";

/** The chat-capable slice of an endpoint catalog matching a search string.
 *  Endpoints that don't annotate routes list everything rather than nothing. */
export function searchChatModels(catalog: OxenModelHit[], query: string): OxenModelHit[] {
  const routed = catalog.some((h) => h.endpoint !== "");
  const chat = routed ? catalog.filter((h) => h.endpoint === "/chat/completions") : catalog;
  const needle = query.trim().toLowerCase();
  if (!needle) return chat;
  return chat.filter((h) =>
    [h.id, h.name, h.developer, h.summary].some((f) => f.toLowerCase().includes(needle)),
  );
}

/** A listing ordered newest release first, so a model the endpoint just
 *  started hosting leads instead of hiding in the alphabet. Undated models
 *  (fine-tunes, older entries) follow, by name. */
export function newestFirst(hits: OxenModelHit[]): OxenModelHit[] {
  return [...hits].sort(
    (a, b) =>
      (b.released_at ?? "").localeCompare(a.released_at ?? "") ||
      a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true }),
  );
}
