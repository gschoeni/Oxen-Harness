import type { ViewModule, ViewTarget } from "../../workbench-sdk";
import { useSyncExternalStore } from "react";
import { workbenchCustomizationEnabled, workflowsEnabled } from "../../lib/features";

const modules = new Map<string, ViewModule>();
const listeners = new Set<() => void>();
let revision = 0;
const changed = () => {
  revision++;
  listeners.forEach((fn) => fn());
};
export function registerView(module: ViewModule) {
  if (modules.has(module.id)) throw new Error(`Duplicate view id: ${module.id}`);
  modules.set(module.id, module);
  changed();
}
export function removeView(id: string) {
  modules.delete(id);
  changed();
}
export function useViewRegistry() {
  useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    () => revision,
  );
  return views();
}
/** Release flags switch views off here rather than at registration, so a
 *  disabled view never lists, resolves a file, or reaches the agent. */
const viewEnabled = (id: string) =>
  id === "workflow"
    ? workflowsEnabled()
    : workbenchCustomizationEnabled() ||
      !(id === "view-studio" || id === "view-manager" || id.startsWith("package:"));

export const views = () => [...modules.values()].filter((view) => viewEnabled(view.id));
export const viewById = (id: string) => viewEnabled(id) ? modules.get(id) : undefined;

/** Old history can reference authoring screens, packages or workflow graphs
 *  disabled this release; a file-backed one reopens in a viewer that is on. */
export function availableTarget(target: ViewTarget): ViewTarget {
  if (viewEnabled(target.view)) return target;
  if ((target.view.startsWith("package:") || target.view === "workflow") && target.path) {
    return { ...target, view: resolveView(target.path) };
  }
  return { view: "welcome" };
}
export function resolveView(path: string) {
  if (path.startsWith("\0diff:")) return "editor";
  return (
    views()
      .filter((view) => view.matches?.(path))
      .sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0))[0]?.id ?? "editor"
  );
}
