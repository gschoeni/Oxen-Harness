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
