import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";
const source = await readFile(new URL("./browser.js", import.meta.url), "utf8");
function host(invoke, development = false) {
  const events = new Map(),
    timers = [];
  const document = {
    addEventListener() {},
    documentElement: { style: { setProperty() {} } },
    body: { textContent: "A view", innerText: "A view" },
  };
  const window = {
    __OXEN_VIEW_CONTEXT__: { development, target: {} },
    __TAURI_INTERNALS__: {
      invoke: (_command, args) => invoke(args.action, args.payload),
    },
    addEventListener: (name, callback) => events.set(name, callback),
  };
  const console = { log() {}, warn() {}, error() {} };
  runInNewContext(source, {
    window,
    document,
    TextEncoder,
    console,
    setTimeout: (callback, delay) => {
      if (delay < 1000) timers.push(callback);
      return 1;
    },
    clearTimeout() {},
    setInterval() {
      return 1;
    },
    clearInterval() {},
  });
  return { api: window.oxenView, events, timers, console };
}
test("retained drafts are serialized, restored after writes, and recover after a failed write", async () => {
  let value = null,
    writes = 0;
  const { api } = host(async (action, payload) => {
    if (action === "retain") {
      writes++;
      if (writes === 1) throw new Error("disk full");
      await new Promise((resolve) => setTimeout(resolve, 1));
      value = payload.value;
    }
    if (action === "restore") return value;
  });
  await assert.rejects(api.retain("first"), /disk full/);
  const second = api.retain("second");
  const third = api.retain("third");
  assert.equal(await api.restore(), "third");
  await Promise.all([second, third]);
});
test("browser tests run only on request and report real failures with a DOM snapshot", async () => {
  const reports = [];
  let pending = null,
    calls = 0;
  const { api, events, timers } = host(async (action, payload) => {
    if (action === "dev_poll") return { test: pending };
    if (action === "dev_test_result") reports.push(payload);
  }, true);
  api.test("fixture", ({ assert }) => {
    calls++;
    assert(false, "fixture did not render");
  });
  events.get("load")();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls, 0);
  pending = { id: "one" };
  await timers.shift()();
  assert.equal(calls, 1);
  assert.equal(reports[0].snapshot, "A view");
  assert.equal(reports[0].results[1].passed, false);
  assert.match(reports[0].results[1].error, /fixture did not render/);
  await timers.shift()();
  assert.equal(calls, 1, "one test request must execute only once");
  events.get("pagehide")();
});
test("successive runtime errors are captured without dropping a burst", async () => {
  const reports = [];
  const { events } = host(async (action, payload) => {
    if (action === "dev_diagnostic") reports.push(payload);
  }, true);
  events.get("error")({ message: "first", filename: "main.js", lineno: 3 });
  events.get("error")({ message: "second", filename: "main.js", lineno: 4 });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(
    reports.map((r) => r.message),
    ["first", "second"],
  );
});

test("runtime errors fail the smoke check and long reports preserve every outcome", async () => {
  const reports = [];
  const { api, events } = host(async (action, payload) => {
    if (action === "dev_poll") return { test: { id: "long" } };
    if (action === "dev_test_result") reports.push(payload);
  }, true);
  for (let i = 0; i < 40; i++)
    api.test(`Unicode failure ${i}`, ({ assert }) =>
      assert(false, "🐂".repeat(1800)),
    );
  events.get("error")({
    message: "script failed",
    filename: "main.js",
    lineno: 1,
  });
  events.get("load")();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(reports.length, 1);
  assert.equal(reports[0].results.length, 41);
  assert.ok(reports[0].results.every((result) => !result.passed));
  assert.ok(
    new TextEncoder().encode(JSON.stringify(reports[0])).length <= 30000,
  );
  events.get("pagehide")();
});

test("a handled failure logged with console.error is reported but passes the smoke check", async () => {
  const diagnostics = [],
    reports = [];
  const { events, console } = host(async (action, payload) => {
    if (action === "dev_diagnostic") diagnostics.push(payload);
    if (action === "dev_poll") return { test: { id: "smoke" } };
    if (action === "dev_test_result") reports.push(payload);
  }, true);
  // A view that catches "no document yet" on first load and renders empty.
  console.error("no document yet", new Error("not found"));
  events.get("load")();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(diagnostics[0].level, "error");
  assert.match(diagnostics[0].message, /no document yet/);
  assert.equal(reports.length, 1);
  assert.equal(reports[0].results[0].passed, true);
  events.get("pagehide")();
});
