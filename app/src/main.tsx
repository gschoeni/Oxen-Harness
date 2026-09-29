import React from "react";
import ReactDOM from "react-dom/client";
import { initUiState } from "./lib/uiState";
import { initFeatureFlags } from "./lib/features";
import "./styles/global.css";

// UI prefs (color mode, dock layout, …) live in ~/.oxen-harness/ui.json and are
// read synchronously at module-init time by the store — so load them BEFORE
// importing anything that pulls the store in. That's why App and the bridges
// are dynamic imports below, not static ones.
async function boot() {
  await initUiState();
  const featureError = await initFeatureFlags().then(
    () => undefined,
    (error) => `Custom work views are unavailable: ${String(error)}`,
  );

  const [{ default: App }, { startAgentEventBridge }, { startCliOpenBridge }, { startLinkRouting }] =
    await Promise.all([
      import("./App"),
      import("./lib/agentEvents"),
      import("./lib/cliOpen"),
      import("./lib/links"),
    ]);

  if (featureError) {
    const { useStore } = await import("./lib/store");
    // Chat notices need a session; App restores it after the first render.
    const unsubscribe = useStore.subscribe((state) => {
      if (!state.session) return;
      unsubscribe();
      state.addNotice(featureError);
    });
  }

  // Subscribe to agent events once, outside React's lifecycle, so StrictMode's
  // double-invoked effects can't register duplicate listeners (which would render
  // every streamed token and tool call twice).
  startAgentEventBridge();
  // Keep link clicks from navigating the main webview away from the app — they
  // open in the link-browser side panel (or the system browser) instead.
  startLinkRouting();
  // Enter the project when `oxen-harness ui <dir>` reaches a running instance.
  startCliOpenBridge();

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void boot();
