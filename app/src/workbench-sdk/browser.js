// Framework-neutral SDK injected into an installed view's dedicated webview.
// The host scopes every request to this instance; no session/root is accepted.
(() => {
  const request = (action, payload = {}) =>
    window.__TAURI_INTERNALS__.invoke("view_bridge", { action, payload });
  const context = window.__OXEN_VIEW_CONTEXT__;
  document.addEventListener(
    "DOMContentLoaded",
    () => {
      for (const [key, value] of Object.entries(context.theme ?? {})) {
        if (key.startsWith("--") && typeof value === "string")
          document.documentElement.style.setProperty(key, value);
      }
    },
    { once: true },
  );
  window.oxenView = Object.freeze({
    apiVersion: 1,
    context,
    request,
    open: (target) => request("open", target),
    addToChat: (text) => request("add_context", { text, path: context.target.path }),
    subscribeFiles: (handler) => {
      if (!context.target.path) return () => {};
      const timer = setInterval(handler, 1000);
      return () => clearInterval(timer);
    },
    read: (path) => request("read", { path }),
    save: (path, content, revision) => request("save", { path, content, revision }),
    asset: (path) => request("asset", { path }),
    report: (state) => request("report", { ...state, path: context.target.path }),
    // Files are authoritative. Polling works across hosts and external edits;
    // stop the subscription when the view no longer needs the document.
    watch: (path, handler, onError) => {
      let revision,
        stopped = false,
        timer;
      async function poll() {
        try {
          const doc = await request("read", { path });
          if (!stopped && doc.revision !== revision) {
            revision = doc.revision;
            handler(doc);
          }
        } catch (error) {
          if (!stopped && onError) onError(error);
        }
        if (!stopped) timer = setTimeout(poll, 1000);
      }
      poll();
      return () => {
        stopped = true;
        clearTimeout(timer);
      };
    },
  });
})();
