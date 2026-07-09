import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { useStore } from "./store";
import * as ipc from "../test/ipcMock";
import { resetAll } from "../test/utils";

beforeEach(resetAll);

describe("store: mode", () => {
  it("toggles light/dark, persisting to the DOM and localStorage", () => {
    useStore.getState().setMode("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(localStorage.getItem("oxen-ui-mode")).toBe("light");

    useStore.getState().toggleMode();
    expect(useStore.getState().mode).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});

describe("store: theme palette", () => {
  it("maps the active theme's primary color onto the accent token", () => {
    useStore.getState().applyTheme(ipc.sampleTheme);
    expect(useStore.getState().theme?.meta.name).toBe("Oregon Trail");
    expect(document.documentElement.style.getPropertyValue("--accent")).toBe(
      ipc.sampleTheme.palette.primary,
    );
    // The link/danger tokens come from the palette too.
    expect(document.documentElement.style.getPropertyValue("--link")).toBe(
      ipc.sampleTheme.palette.link,
    );
  });
});

describe("store: sessions", () => {
  it("startNewSession swaps in a fresh session with an empty thread", async () => {
    await useStore.getState().startNewSession();
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(useStore.getState().session?.session_id).toBe("new-session-id");
    expect(useStore.getState().threads["new-session-id"]).toEqual([]);
    expect(ipc.listSessions).toHaveBeenCalled(); // refreshed history
  });

  it("resume seeds the thread from a cold session's transcript", async () => {
    ipc.resumeSession.mockResolvedValueOnce({
      info: { ...ipc.sampleSession, session_id: "abc" },
      messages: [
        { role: "user", content: "hi" },
        { role: "assistant", content: "hello" },
      ],
      running: false,
    });
    await useStore.getState().resume("abc");
    expect(ipc.resumeSession).toHaveBeenCalledWith("abc");
    expect(useStore.getState().session?.session_id).toBe("abc");
    expect(useStore.getState().threads["abc"]).toHaveLength(2);
  });

  it("switching to a running chat keeps its live thread and clears its unread dot", async () => {
    // A background chat that already streamed a thread and finished unread.
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "current" },
      infos: { bg: { ...ipc.sampleSession, session_id: "bg" } },
      threads: { bg: [{ id: "1", kind: "assistant", text: "live", streaming: false }] },
      runStatus: { bg: "unread" },
    });
    // Backend reports a mid-turn chat with running=true / empty transcript.
    ipc.resumeSession.mockResolvedValueOnce({
      info: { model: "", workspace: "", session_id: "bg", tokens_used: 0, context_tokens: 0, context_window: 0, compression_mode: "off" },
      messages: [],
      running: true,
    });
    await useStore.getState().resume("bg");
    expect(useStore.getState().session?.session_id).toBe("bg");
    // The live thread is preserved (not clobbered by the empty transcript).
    expect(useStore.getState().threads["bg"]).toHaveLength(1);
    // Viewing it marks it read.
    expect(useStore.getState().runStatus["bg"]).toBeUndefined();
  });

  it("send marks a finished off-screen chat as unread", async () => {
    let finishTurn!: (v: string) => void;
    ipc.runTurn.mockImplementationOnce(() => new Promise((r) => (finishTurn = r)));
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "bg" },
      infos: { bg: { ...ipc.sampleSession, session_id: "bg" } },
      threads: { bg: [] },
    });

    useStore.getState().send("do a thing");
    expect(ipc.runTurn).toHaveBeenCalledWith("bg", "do a thing", []);
    expect(useStore.getState().runStatus["bg"]).toBe("running");

    // Switch away while it's still running, then let it finish.
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "other" } });
    finishTurn("Done.");
    await vi.waitFor(() => expect(useStore.getState().runStatus["bg"]).toBe("unread"));
  });

  it("preserves attachments on prompts queued behind a running turn", async () => {
    let finishFirst!: (v: string) => void;
    ipc.runTurn
      .mockImplementationOnce(() => new Promise((r) => (finishFirst = r)))
      .mockResolvedValueOnce("Queued done.");

    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s1" },
      infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
      threads: { s1: [] },
    });

    useStore.getState().send("first");
    useStore.getState().send("second", ["/tmp/diagram.png"]);

    expect(useStore.getState().queues["s1"]).toEqual([
      { text: "second", attachments: ["/tmp/diagram.png"] },
    ]);

    finishFirst("First done.");
    await vi.waitFor(() =>
      expect(ipc.runTurn).toHaveBeenLastCalledWith("s1", "second", ["/tmp/diagram.png"]),
    );
  });

  it("keeps hidden queue attachments when the visible text list is unchanged", () => {
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s1" },
      queues: {
        s1: [
          { text: "with file", attachments: ["/tmp/file.pdf"] },
          { text: "plain", attachments: [] },
        ],
      },
    });

    useStore.getState().setQueue(["with file", "plain"]);
    expect(useStore.getState().queues["s1"][0].attachments).toEqual(["/tmp/file.pdf"]);
  });

  it("resume is a no-op when the target is already the active session", async () => {
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "same" } });
    await useStore.getState().resume("same");
    expect(ipc.resumeSession).not.toHaveBeenCalled();
  });
});

describe("store: compression", () => {
  it("ingestCompression tracks the session's running savings and latest mode", () => {
    useStore.getState().ingestCompression({
      session: "s1",
      mode: "audit",
      saved_tokens: 500,
      total_saved_tokens: 500,
      results_compressed: 2,
    });
    expect(useStore.getState().compression["s1"]).toEqual({ mode: "audit", tokensSaved: 500 });

    // A later event supersedes the counters (the payload carries the running total).
    useStore.getState().ingestCompression({
      session: "s1",
      mode: "on",
      saved_tokens: 700,
      total_saved_tokens: 1200,
      results_compressed: 3,
    });
    expect(useStore.getState().compression["s1"]).toEqual({ mode: "on", tokensSaved: 1200 });
  });

  it("ingestCompression keeps sessions independent and never touches the thread", () => {
    useStore.setState({ threads: { s1: [] } });
    useStore.getState().ingestCompression({
      session: "s1",
      mode: "on",
      saved_tokens: 10,
      total_saved_tokens: 10,
      results_compressed: 1,
    });
    useStore.getState().ingestCompression({
      session: "s2",
      mode: "audit",
      saved_tokens: 20,
      total_saved_tokens: 20,
      results_compressed: 1,
    });
    expect(useStore.getState().compression["s1"]).toEqual({ mode: "on", tokensSaved: 10 });
    expect(useStore.getState().compression["s2"]).toEqual({ mode: "audit", tokensSaved: 20 });
    // No per-event thread notice — it fires every model call.
    expect(useStore.getState().threads["s1"]).toEqual([]);
  });
});

describe("store: local model load status", () => {
  it("creates the switch state for a load it didn't initiate (startup restore)", () => {
    // No switchToLocalModel ran — the event alone must surface the loading UI.
    useStore.getState().setLocalStatus({ model: "qwen3-1.7b", phase: "starting" });
    expect(useStore.getState().localSwitch).toMatchObject({
      model: "qwen3-1.7b",
      phase: "starting",
    });

    // Later phases update in place, keeping the original start time.
    const started = useStore.getState().localSwitch!.startedAt;
    useStore.getState().setLocalStatus({ model: "qwen3-1.7b", phase: "loading" });
    expect(useStore.getState().localSwitch).toMatchObject({
      phase: "loading",
      startedAt: started,
    });
  });

  it("clears the switch state when the load ends (ready or error)", () => {
    useStore.getState().setLocalStatus({ model: "qwen3-1.7b", phase: "loading" });
    useStore.getState().setLocalStatus({ model: "qwen3-1.7b", phase: "ready" });
    expect(useStore.getState().localSwitch).toBeNull();

    // A load that dies mid-way (backend emits "error") must not stick either.
    useStore.getState().setLocalStatus({ model: "qwen3-1.7b", phase: "starting" });
    useStore.getState().setLocalStatus({ model: "qwen3-1.7b", phase: "error" });
    expect(useStore.getState().localSwitch).toBeNull();
  });
});

describe("store: code review", () => {
  // startCodeReview needs a current session with a thread to write into.
  const seedSession = () =>
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s1" },
      infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
      threads: { s1: [] },
    });

  it("runs a review, lands the exchange in the thread, and clears its state", async () => {
    seedSession();
    useStore.getState().startCodeReview();
    // The card and running status appear synchronously, before the IPC settles.
    expect(useStore.getState().codeReview["s1"]).toBeDefined();
    expect(useStore.getState().runStatus["s1"]).toBe("running");
    expect(ipc.runCodeReview).toHaveBeenCalledWith("s1", undefined);

    // Once the (mocked) review resolves: the user+assistant pair is appended,
    // the card is cleared, and the chat settles back to read (it's in view).
    await vi.waitFor(() => {
      const thread = useStore.getState().threads["s1"];
      expect(thread.some((it) => it.kind === "assistant" && it.text.includes("no findings"))).toBe(
        true,
      );
    });
    expect(useStore.getState().codeReview["s1"]).toBeUndefined();
    expect(useStore.getState().fleets["s1"]).toBeUndefined();
    expect(useStore.getState().runStatus["s1"]).toBeUndefined();
  });

  it("passes a base branch through to the backend", () => {
    seedSession();
    useStore.getState().startCodeReview("main");
    expect(ipc.runCodeReview).toHaveBeenCalledWith("s1", "main");
  });

  it("won't start a second review while the chat is already running", () => {
    seedSession();
    useStore.setState({ runStatus: { s1: "running" } });
    useStore.getState().startCodeReview();
    expect(ipc.runCodeReview).not.toHaveBeenCalled();
  });

  it("a nothing-to-review result leaves a notice, not an exchange", async () => {
    seedSession();
    ipc.runCodeReview.mockResolvedValueOnce({
      status: "nothing",
      user: "",
      assistant: "",
      findings: 0,
      tokens_used: 0,
    });
    useStore.getState().startCodeReview();
    await vi.waitFor(() => {
      const thread = useStore.getState().threads["s1"];
      expect(thread.some((it) => it.kind === "notice" && /no changes/i.test(it.text))).toBe(true);
    });
  });

  it("progress ingestion only updates a live card, and activity is capped", () => {
    seedSession();
    // No card yet: a stray progress event is ignored.
    useStore.getState().ingestCodeReviewProgress({
      session: "s1",
      step: "find",
      index: 0,
      total: 3,
      agents: [],
    });
    expect(useStore.getState().codeReview["s1"]).toBeUndefined();

    // With a card, progress advances it and activity rolls with a cap.
    useStore.setState({ codeReview: { s1: { step: "", index: 0, total: 0, activity: "" } } });
    useStore.getState().ingestCodeReviewProgress({
      session: "s1",
      step: "verify",
      index: 1,
      total: 3,
      agents: [],
    });
    expect(useStore.getState().codeReview["s1"]).toMatchObject({ step: "verify", index: 1 });

    useStore.getState().ingestCodeReviewActivity("s1", "x".repeat(500), false);
    expect(useStore.getState().codeReview["s1"]!.activity.length).toBeLessThanOrEqual(120);
  });
});
