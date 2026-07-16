import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { startAgentEventBridge } from "./lib/agentEvents";
import { startLinkRouting } from "./lib/links";
import "./styles/global.css";

// Subscribe to agent events once, outside React's lifecycle, so StrictMode's
// double-invoked effects can't register duplicate listeners (which would render
// every streamed token and tool call twice).
startAgentEventBridge();
// Keep link clicks from navigating the main webview away from the app — they
// open in the link-browser side panel (or the system browser) instead.
startLinkRouting();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
