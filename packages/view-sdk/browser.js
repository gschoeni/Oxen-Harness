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
  const tests = new Map();
  let retention = Promise.resolve();
  window.oxenView = Object.freeze({
    apiVersion: 1,
    context,
    request,
    retain: (value) => {
      retention = retention.then(
        () => request("retain", { value }),
        () => request("retain", { value }),
      );
      return retention;
    },
    restore: async () => {
      await retention;
      return request("restore");
    },
    test: (name, run) => {
      if (tests.size >= 49 || typeof name !== "string" || typeof run !== "function")
        throw new Error("A view can register up to 49 named test functions");
      tests.set(name, run);
    },
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

  if (context.development) {
    let closed = false,
      timer,
      seenTest,
      reports = 0;
    const describe = (value) => {
      if (value instanceof Error) return value.stack || value.message;
      if (typeof value === "string") return value;
      try {
        return JSON.stringify(value);
      } catch {
        return String(value);
      }
    };
    const diagnostic = (level, message, file = null, line = null) => {
      if (closed || reports++ >= 100) return;
      void request("dev_diagnostic", {
        level,
        message: String(message).slice(0, 2000),
        file,
        line,
      }).catch((error) => originalError("View diagnostics unavailable:", error));
    };
    const originalError = console.error.bind(console);
    for (const level of ["log", "warn", "error"]) {
      const original = console[level].bind(console);
      console[level] = (...args) => {
        original(...args);
        diagnostic(level, args.map(describe).join(" "));
      };
    }
    window.addEventListener(
      "error",
      (event) =>
        diagnostic(
          "error",
          event.message ||
            `Failed to load ${event.target?.src || event.target?.href || "resource"}`,
          event.filename || null,
          event.lineno || null,
        ),
      true,
    );
    window.addEventListener("unhandledrejection", (event) =>
      diagnostic("error", describe(event.reason)),
    );
    window.addEventListener("securitypolicyviolation", (event) =>
      diagnostic(
        "error",
        `Blocked by package policy: ${event.violatedDirective} ${event.blockedURI}`,
      ),
    );
    window.addEventListener("pagehide", () => {
      closed = true;
      clearTimeout(timer);
    });
    async function runTests(test) {
      const results = [{ name: "Preview rendered", passed: !!document.body?.textContent?.trim() }];
      const deadline = Date.now() + 24000;
      for (const [name, run] of tests) {
        if (Date.now() >= deadline) {
          results.push({ name, passed: false, error: "The test suite exceeded 24 seconds" });
          break;
        }
        let timeout;
        try {
          await Promise.race([
            Promise.resolve().then(() =>
              run({
                assert: (condition, message = "Assertion failed") => {
                  if (!condition) throw new Error(message);
                },
              }),
            ),
            new Promise((_, reject) => {
              timeout = setTimeout(
                () => reject(new Error("Test timed out after 5 seconds")),
                Math.min(5000, deadline - Date.now()),
              );
            }),
          ]);
          results.push({ name, passed: true });
        } catch (error) {
          results.push({ name, passed: false, error: describe(error).slice(0, 2000) });
        } finally {
          clearTimeout(timeout);
        }
      }
      await request("dev_test_result", {
        id: test.id,
        results,
        snapshot: document.body?.innerText?.slice(0, 8000) ?? "",
      });
    }
    async function poll() {
      try {
        const { test } = await request("dev_poll");
        if (test && seenTest !== test.id) {
          seenTest = test.id;
          await runTests(test);
        }
      } catch (error) {
        if (!closed) originalError("View Studio connection:", error);
      }
      if (!closed) timer = setTimeout(poll, 750);
    }
    window.addEventListener(
      "load",
      () => {
        void request("dev_ready")
          .then(poll)
          .catch((error) => originalError("View Studio ready:", error));
      },
      { once: true },
    );
  }
})();
