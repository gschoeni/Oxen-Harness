// A chat's standing as one small dot, in the state's color. The tab strip and
// the history rows both wear it, so the same color means the same thing on
// either. Idle is a faint neutral dot rather than nothing: the slot stays
// put when a chat's state changes, and the row keeps its rhythm.

import type { TabState } from "./tabStatus";
import "./tabs.css";

export function StatusDot({ state, label }: { state: TabState; label?: string | null }) {
  return (
    <span
      className={`status-dot ${state}`}
      role={state === "idle" ? undefined : "img"}
      aria-label={state === "idle" ? undefined : (label ?? state)}
      aria-hidden={state === "idle" || undefined}
    />
  );
}
