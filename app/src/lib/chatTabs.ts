// The tab layout as data: workspace path → session ids in strip order.
//
// Pure functions over that record, so the store's actions read as intent
// ("close this tab, land on its neighbour") and the arithmetic is tested on
// its own. Every function returns the same object when nothing changes, so a
// caller can skip the persist.

export type ChatTabs = Record<string, string[]>;

/** The persisted layout, or `{}` for anything that isn't the expected shape —
 *  a hand-edited ui.json is ignored rather than trusted. */
export function parseChatTabs(raw: unknown): ChatTabs {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  return Object.fromEntries(
    Object.entries(raw as Record<string, unknown>).filter(
      (entry): entry is [string, string[]] =>
        Array.isArray(entry[1]) && entry[1].every((id) => typeof id === "string"),
    ),
  );
}

/** `tabs` with `id` open in `workspace`'s strip, appended when new. */
export function withTab(tabs: ChatTabs, workspace: string, id: string): ChatTabs {
  const ids = tabs[workspace] ?? [];
  if (ids.includes(id)) return tabs;
  return { ...tabs, [workspace]: [...ids, id] };
}

/** `tabs` without `ids`, in every project's strip; a strip left empty is
 *  dropped. */
export function withoutTabs(tabs: ChatTabs, ids: Iterable<string>): ChatTabs {
  const gone = new Set(ids);
  let changed = false;
  const next: ChatTabs = {};
  for (const [workspace, list] of Object.entries(tabs)) {
    const kept = list.filter((id) => !gone.has(id));
    if (kept.length !== list.length) changed = true;
    if (kept.length) next[workspace] = kept;
    else changed = true;
  }
  return changed ? next : tabs;
}

/** `tabs` without `workspace`'s strip altogether (its project was removed). */
export function withoutStrip(tabs: ChatTabs, workspace: string): ChatTabs {
  if (!(workspace in tabs)) return tabs;
  const { [workspace]: _gone, ...rest } = tabs;
  return rest;
}

/** `tabs` with `id` moved within its strip to sit just before `before`, or
 *  at the end when `before` is null. Both must be in the same strip. */
export function movedTab(tabs: ChatTabs, id: string, before: string | null): ChatTabs {
  const strip = stripOf(tabs, id);
  if (!strip || before === id) return tabs;
  const [workspace, ids] = strip;
  if (before !== null && !ids.includes(before)) return tabs;
  const without = ids.filter((x) => x !== id);
  const at = before === null ? without.length : without.indexOf(before);
  const next = [...without.slice(0, at), id, ...without.slice(at)];
  if (next.every((x, i) => x === ids[i])) return tabs;
  return { ...tabs, [workspace]: next };
}

/** Where the strip lands when `id` closes: the tab to its right, else the
 *  one to its left, else nowhere (it was alone or not there). */
export function neighbourTab(ids: string[], id: string): string | null {
  const i = ids.indexOf(id);
  if (i < 0) return null;
  return ids[i + 1] ?? ids[i - 1] ?? null;
}

/** The strip `id` lives in, if any. */
export function stripOf(tabs: ChatTabs, id: string): [string, string[]] | undefined {
  return Object.entries(tabs).find(([, ids]) => ids.includes(id));
}
