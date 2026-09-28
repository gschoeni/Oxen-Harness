// Reconstruct the task plan from an `update_plan` tool call's raw arguments.
// The plan lives in the tool call, so it survives a resumed chat with no stored state.

import type { PlanItem, PlanStatus } from "./types";

const STATUSES: PlanStatus[] = ["pending", "in_progress", "completed"];

/** Parse `update_plan` tool args into a plan, or null if there are no valid
 *  items (e.g. a malformed/partial call). */
export function planItemsFromArgs(a: Record<string, unknown>): PlanItem[] | null {
  const raw = a.plan;
  if (!Array.isArray(raw)) return null;
  const items: PlanItem[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const e = entry as Record<string, unknown>;
    const content = typeof e.content === "string" ? e.content.trim() : "";
    if (!content) continue;
    const activeForm = typeof e.active_form === "string" && e.active_form.trim() ? e.active_form.trim() : content;
    const status = STATUSES.includes(e.status as PlanStatus) ? (e.status as PlanStatus) : "pending";
    items.push({ content, active_form: activeForm, status });
  }
  return items.length ? items : null;
}

/** Completed / total counts for a plan. */
export function planProgress(items: PlanItem[]): { done: number; total: number } {
  return {
    done: items.filter((i) => i.status === "completed").length,
    total: items.length,
  };
}
