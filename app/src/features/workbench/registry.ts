import type { ViewModule } from "../../workbench-sdk";
import { useSyncExternalStore } from "react";

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
export const views = () => [...modules.values()];
export const viewById = (id: string) => modules.get(id);
export function resolveView(path: string) {
  if (path.startsWith("\0diff:")) return "editor";
  return (
    views()
      .filter((view) => view.matches?.(path))
      .sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0))[0]?.id ?? "editor"
  );
}
