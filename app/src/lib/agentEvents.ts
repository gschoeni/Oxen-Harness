// Single source of truth for agent event subscriptions.
//
// These are global, app-lifetime streams (tokens, tool activity, usage, canvas,
// clarifying questions). Subscribing from a React effect is fragile: StrictMode
// double-invokes effects and Tauri's `listen` is async, so the obvious pattern
// races into DUPLICATE listeners — which makes every streamed token and tool
// call render twice. Instead we subscribe exactly once here, dispatching into
// the store via `getState()`, guarded so repeated calls (StrictMode) and module
// re-evaluation (HMR) never register a second set of listeners.

import type { UnlistenFn } from "@tauri-apps/api/event";
import { useStore } from "./store";
import {
  onCanvas,
  onCanvasWriting,
  onCodeReviewProgress,
  onCodeReviewToken,
  onCodeReviewTool,
  onFleetActivity,
  onFleetAgent,
  onFleetCompleted,
  onFleetStarted,
  onLocalStatus,
  onQuestion,
  onToken,
  onTool,
  onToolDelta,
  onUsage,
  onCompacted,
  onCompression,
  onRetry,
} from "./ipc";

// Stored on `window` so the guard survives HMR module re-evaluation (a fresh
// module instance still sees the existing subscription and won't double up).
declare global {
  interface Window {
    __oxenAgentBridge?: boolean;
  }
}

let unlisteners: UnlistenFn[] = [];

/** Subscribe to all agent events exactly once. Safe to call repeatedly. */
export function startAgentEventBridge(): void {
  if (window.__oxenAgentBridge) return;
  window.__oxenAgentBridge = true;

  const s = () => useStore.getState();
  const pending = [
    onToken((e) => s().ingestToken(e.session, e.token)),
    onTool((e) => s().ingestTool(e)),
    onToolDelta((e) => s().ingestToolDelta(e)),
    onUsage((e) => s().ingestUsage(e)),
    onCompacted((e) => s().ingestCompacted(e)),
    onCompression((e) => s().ingestCompression(e)),
    onRetry((e) => s().ingestRetry(e)),
    onCanvas((e) => s().ingestCanvas(e)),
    onCanvasWriting((session) => s().setCanvasWriting(session, true)),
    onQuestion((q) => s().setQuestion(q)),
    onLocalStatus((e) => s().setLocalStatus(e)),
    onCodeReviewProgress((e) => s().ingestCodeReviewProgress(e)),
    onCodeReviewToken((e) => s().ingestCodeReviewActivity(e.session, e.token, false)),
    onCodeReviewTool((e) => s().ingestCodeReviewActivity(e.session, `⚙ ${e.name}…`, true)),
    onFleetStarted((e) => s().ingestFleetStarted(e)),
    onFleetAgent((e) => s().ingestFleetAgent(e)),
    onFleetActivity((e) => s().ingestFleetActivity(e)),
    onFleetCompleted((session) => s().ingestFleetCompleted(session)),
  ];
  Promise.all(pending).then((fns) => {
    unlisteners = fns;
  });
}

// On HMR dispose, tear the listeners down and clear the guard so the next module
// evaluation re-subscribes cleanly (rather than leaking a stale set).
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    window.__oxenAgentBridge = false;
    unlisteners.forEach((fn) => fn());
    unlisteners = [];
  });
}
