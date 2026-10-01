import type { ViewTarget } from "../../workbench-sdk";

export interface WorkContext {
  current: ViewTarget;
  history: ViewTarget[];
  cursor: number;
}
export function navigate(previous: WorkContext | undefined, target: ViewTarget): WorkContext {
  if (previous && JSON.stringify(previous.current) === JSON.stringify(target)) return previous;
  const history = [...(previous?.history.slice(0, previous.cursor + 1) ?? []), target].slice(-50);
  return {
    current: target,
    history,
    cursor: history.length - 1,
  };
}
export function travel(previous: WorkContext, offset: number): WorkContext {
  const cursor = Math.max(0, Math.min(previous.history.length - 1, previous.cursor + offset));
  return { ...previous, cursor, current: previous.history[cursor] };
}
