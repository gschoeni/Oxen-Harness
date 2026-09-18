import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { useStore } from "./store";
import { getUi } from "./uiState";
import * as ipc from "../test/ipcMock";
import { resetAll } from "../test/utils";
import type { LedgerEntry } from "./types";

beforeEach(resetAll);

describe("store: mode", () => {
  it("toggles light/dark, persisting to the DOM and the UI prefs", () => {
    useStore.getState().setMode("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(getUi("mode")).toBe("light");

    useStore.getState().toggleMode();
    expect(useStore.getState().mode).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});

describe("store: editor git state + wrap", () => {
  it("toggleEditorWrap flips and persists the preference", () => {
    expect(useStore.getState().editorWrap).toBe(false);
    useStore.getState().toggleEditorWrap();
    expect(useStore.getState().editorWrap).toBe(true);
    expect(getUi("editorWrap")).toBe(true);
    useStore.getState().toggleEditorWrap();
    expect(useStore.getState().editorWrap).toBe(false);
  });

  it("refreshGitStatus caches per workspace and survives a failed probe", async () => {
    ipc.gitStatus.mockResolvedValueOnce([
      { path: "a.txt", original_path: null, status: "modified", index: " ", worktree: "M" },
    ]);
    await useStore.getState().refreshGitStatus("/ws");
    expect(useStore.getState().gitStates["/ws"]).toHaveLength(1);

    // A failure keeps the last known state rather than erasing the badges.
    ipc.gitStatus.mockRejectedValueOnce(new Error("git exploded"));
    await useStore.getState().refreshGitStatus("/ws");
    expect(useStore.getState().gitStates["/ws"]).toHaveLength(1);

    // A non-repo workspace records null — its UI hides, others keep theirs.
    ipc.gitStatus.mockResolvedValueOnce(null);
    await useStore.getState().refreshGitStatus("/elsewhere");
    expect(useStore.getState().gitStates["/elsewhere"]).toBeNull();
    expect(useStore.getState().gitStates["/ws"]).toHaveLength(1);
  });
});

describe("store: navigation", () => {
  it("starts at the Ledger, the application's navigation root", () => {
    expect(useStore.getState().homeOpen).toBe(true);
  });

  it("riding out of the Ledger records the visit; opening refreshes the board", async () => {
    useStore.getState().setHomeOpen(false);
    expect(ipc.ledgerMarkSeen).toHaveBeenCalled();

    useStore.getState().setHomeOpen(true);
    await vi.waitFor(() => expect(ipc.ledgerSnapshot).toHaveBeenCalled());
  });

  it("targets a project home explicitly and clears that target for the project list", () => {
    useStore.getState().openProjectHome("/work/demo");
    expect(useStore.getState().homeOpen).toBe(true);
    expect(useStore.getState().projectHomePath).toBe("/work/demo");

    useStore.getState().setHomeOpen(true);
    expect(useStore.getState().projectHomePath).toBeNull();
  });
});

describe("store: per-thread seen marks", () => {
  it("opening a chat marks it seen, then repaints the board against the new mark", async () => {
    await useStore.getState().resume("older");
    await vi.waitFor(() => expect(ipc.sessionMarkSeen).toHaveBeenCalledWith("older"));
    await vi.waitFor(() => expect(ipc.ledgerSnapshot).toHaveBeenCalled());
    // The mark lands BEFORE the snapshot that repaints — never the other way.
    const markAt = ipc.sessionMarkSeen.mock.invocationCallOrder[0];
    const order = ipc.ledgerSnapshot.mock.invocationCallOrder;
    const lastSnapshot = order[order.length - 1];
    expect(lastSnapshot).toBeGreaterThan(markAt);
  });
});

describe("store: trail dust", () => {
  it("tool starts raise dust for any session, cached thread or not", () => {
    // No thread cached for "bg" — the chat runs entirely in the background —
    // yet the Ledger still sees its wagon working.
    useStore.getState().ingestTool({ session: "bg", name: "read_file", phase: "start", detail: "" });
    useStore.getState().ingestTool({ session: "bg", name: "read_file", phase: "end", detail: "" });
    useStore.getState().ingestTool({ session: "bg", name: "git", phase: "start", detail: "" });
    expect(useStore.getState().trailDust.bg).toBe(2);
  });

  it("tool starts keep the riding-now readout current for background sessions", () => {
    useStore
      .getState()
      .ingestTool({ session: "bg", name: "read_file", phase: "start", detail: "src/a.rs" });
    expect(useStore.getState().trailActivity.bg?.name).toBe("read_file");
    useStore
      .getState()
      .ingestTool({ session: "bg", name: "run_shell", phase: "start", detail: "cargo test" });
    const activity = useStore.getState().trailActivity.bg;
    expect(activity?.name).toBe("run_shell");
    expect(activity?.detail).toBe("cargo test");
  });
});

describe("store: ledger refreshes", () => {
  const entry = (id: string, workspace: string, settled = false): import("./types").LedgerEntry => ({
    id,
    workspace,
    model: "m",
    created_at: 1,
    last_activity_at: 1,
    title: "t",
    last_reply: "",
    message_count: 1,
    mid_turn: false,
    seen_at: 0,
    plan: null,
    trail: null,
    review_status: "",
    settle: settled ? { settled_at: 1, note: "" } : null,
  });

  it("a landed update_trail repaints the board even with the chat open (Home closed)", async () => {
    useStore.setState({ homeOpen: false });
    vi.mocked(ipc.ledgerSnapshot).mockClear();
    useStore.getState().ingestTool({ session: "bg", name: "update_trail", phase: "end", detail: "" });
    await vi.waitFor(() => expect(ipc.ledgerSnapshot).toHaveBeenCalled());
  });

  it("an older refresh's git probe never overwrites a newer board", async () => {
    vi.mocked(ipc.ledgerSnapshot).mockResolvedValue({
      entries: [entry("s1", "/work/app")],
      running: [],
      last_seen: 0,
    });
    // The older refresh's git probe hangs (a slow repo) while a newer refresh
    // starts, completes with the pushed state, and paints. The stale probe
    // then resolves LAST with pre-push state — it must be discarded.
    let releaseOld: (v: Record<string, unknown>) => void = () => {};
    vi.mocked(ipc.workspaceGit).mockImplementationOnce(
      () => new Promise((resolve) => (releaseOld = resolve)) as never,
    );
    const older = useStore.getState().refreshLedger();
    await vi.waitFor(() => expect(ipc.workspaceGit).toHaveBeenCalled());

    vi.mocked(ipc.workspaceGit).mockResolvedValueOnce({
      "/work/app": { branch: "main", dirty_files: 0, ahead: 0, behind: 0, has_upstream: true },
    } as never);
    await useStore.getState().refreshLedger();
    expect(useStore.getState().ledgerGit["/work/app"]?.ahead).toBe(0);

    releaseOld({
      "/work/app": { branch: "main", dirty_files: 0, ahead: 1, behind: 0, has_upstream: true },
    });
    await older;
    // The stale probe was discarded — the board still shows the pushed state.
    expect(useStore.getState().ledgerGit["/work/app"]?.ahead).toBe(0);
  });

  it("a workspace whose threads all settled sheds its git banner", async () => {
    vi.mocked(ipc.ledgerSnapshot).mockResolvedValue({
      entries: [entry("s1", "/work/app")],
      running: [],
      last_seen: 0,
    });
    vi.mocked(ipc.workspaceGit).mockResolvedValue({
      "/work/app": { branch: "main", dirty_files: 1, ahead: 0, behind: 0, has_upstream: true },
    } as never);
    await useStore.getState().refreshLedger();
    expect(useStore.getState().ledgerGit["/work/app"]).toBeDefined();

    vi.mocked(ipc.ledgerSnapshot).mockResolvedValue({
      entries: [entry("s1", "/work/app", true)],
      running: [],
      last_seen: 0,
    });
    await useStore.getState().refreshLedger();
    expect(useStore.getState().ledgerGit).toEqual({});
    expect(ipc.workspaceGit).toHaveBeenCalledTimes(1); // no probe for settled-only
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

describe("store: token coalescing", () => {
  const seed = (id = "s1") =>
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: id },
      infos: { [id]: { ...ipc.sampleSession, session_id: id } },
      threads: { [id]: [{ id: "a", kind: "assistant", text: "", streaming: true }] },
    });
  const text = (id = "s1") =>
    (useStore.getState().threads[id] ?? [])
      .flatMap((it) => (it.kind === "assistant" ? [it.text] : []))
      .join("|");

  it("buffers a burst and lands it as one thread update on the timer", () => {
    vi.useFakeTimers();
    try {
      seed();
      const before = useStore.getState().threads.s1;
      useStore.getState().ingestToken("s1", "Hel");
      useStore.getState().ingestToken("s1", "lo");
      useStore.getState().ingestToken("s1", ", world");
      // Nothing has touched the store yet — same array identity.
      expect(useStore.getState().threads.s1).toBe(before);
      expect(useStore.getState().liveTokens.s1 ?? 0).toBe(0);

      let renders = 0;
      const unsub = useStore.subscribe(() => renders++);
      vi.advanceTimersByTime(50);
      unsub();
      expect(renders).toBe(1); // three tokens, one copy
      expect(text()).toBe("Hello, world");
      expect(useStore.getState().liveTokens.s1).toBeCloseTo("Hello, world".length / 4);
    } finally {
      vi.useRealTimers();
    }
  });

  it("flushes pending tokens ahead of any other store mutation, keeping order", () => {
    vi.useFakeTimers();
    try {
      seed();
      useStore.getState().ingestToken("s1", "I'll look");
      // A tool start arrives before the timer: the preamble must settle into
      // its bubble BEFORE the chip, exactly as if every token had landed alone.
      useStore.getState().ingestTool({ session: "s1", name: "read_file", phase: "start", detail: "{}" });
      expect(useStore.getState().threads.s1.map((it) => it.kind)).toEqual(["assistant", "tool"]);
      expect(useStore.getState().threads.s1[0]).toMatchObject({ text: "I'll look", streaming: false });
      expect(vi.getTimerCount()).toBe(0); // the flush cancelled its own timer

      // Tokens after the tool end land in the fresh bubble, not the old one.
      useStore.getState().ingestTool({ session: "s1", name: "read_file", phase: "end", detail: "ok" });
      useStore.getState().ingestToken("s1", "Found it");
      useStore.getState().flushTokens();
      expect(text()).toBe("I'll look|Found it");
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps sessions apart and drops tokens for a session with no cached thread", () => {
    vi.useFakeTimers();
    try {
      seed("a");
      useStore.setState({
        threads: {
          a: [{ id: "a", kind: "assistant", text: "", streaming: true }],
          b: [{ id: "b", kind: "assistant", text: "", streaming: true }],
        },
      });
      useStore.getState().ingestToken("a", "A1");
      useStore.getState().ingestToken("b", "B1");
      useStore.getState().ingestToken("evicted", "lost");
      useStore.getState().ingestToken("a", "A2");
      vi.advanceTimersByTime(50);
      expect(text("a")).toBe("A1A2");
      expect(text("b")).toBe("B1");
      // No stub thread is conjured for a session that isn't cached (same rule
      // the per-event path always applied).
      expect(useStore.getState().threads.evicted).toBeUndefined();
      expect(useStore.getState().liveTokens.evicted).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a turn's end never races its own tokens", async () => {
    let finishTurn!: (v: string) => void;
    ipc.runTurn.mockImplementationOnce(() => new Promise((r) => (finishTurn = r)));
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s1" },
      infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
      threads: { s1: [] },
    });
    useStore.getState().send("go");
    useStore.getState().ingestToken("s1", "streamed reply");
    finishTurn("streamed reply"); // the backend returns the same text it streamed
    await vi.waitFor(() => expect(useStore.getState().runStatus.s1).toBeUndefined());
    // The buffered text settled into the bubble (not lost, not duplicated).
    expect(text()).toBe("streamed reply");
  });
});

describe("store: thread cache under many running sessions", () => {
  const info = (id: string) => ({ ...ipc.sampleSession, session_id: id });
  const bubble = (id: string): import("../features/chat/thread").Item[] => [
    { id: `${id}-a`, kind: "assistant", text: `${id} says`, streaming: true },
  ];

  it("protects only the most recently active running sessions (plus the current one)", async () => {
    // Nine background chats mid-turn — a fleet — plus the one in view.
    const ids = ["r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9"];
    useStore.setState({
      session: info("cur"),
      infos: Object.fromEntries([...ids, "cur"].map((id) => [id, info(id)])),
      threads: Object.fromEntries([...ids, "cur"].map((id) => [id, bubble(id)])),
      runStatus: Object.fromEntries(ids.map((id) => [id, "running" as const])),
      queues: { r1: [{ text: "later", attachments: [] }] },
    });
    // r1 is the oldest by insertion but just swung a tool — activity ranks it
    // first; r2/r3 (old, idle) are the ones released.
    useStore.getState().ingestTool({ session: "r1", name: "git", phase: "start", detail: "" });
    await useStore.getState().startNewSession();

    const kept = Object.keys(useStore.getState().threads);
    expect(kept).toContain("new-session-id");
    expect(kept).toContain("r1");
    expect(kept).not.toContain("r2");
    expect(kept).not.toContain("r3");
    expect(kept.filter((id) => id.startsWith("r"))).toHaveLength(6);
    // "cur" is no longer current and not running: it's a plain cached thread,
    // and the cap is already spent on protected ones.
    expect(kept).not.toContain("cur");

    // The evicted chats are still running: their live state stays put — only
    // the (rebuildable) thread was released.
    expect(useStore.getState().infos.r2).toBeDefined();
    expect(useStore.getState().queues.r1).toHaveLength(1);
    expect(useStore.getState().runStatus.r2).toBe("running");

    // Tokens for an evicted session are dropped safely — no stub thread.
    useStore.getState().ingestToken("r2", "still going");
    useStore.getState().flushTokens();
    expect(useStore.getState().threads.r2).toBeUndefined();
  });

  it("a running chat whose thread was released rebuilds it from the persisted transcript when viewed", async () => {
    useStore.setState({
      session: info("cur"),
      infos: { cur: info("cur"), bg: info("bg") },
      threads: { cur: [] },
      runStatus: { bg: "running" },
    });
    // Mid-turn: the view can't read the agent, but the DB can be read.
    ipc.resumeSession.mockResolvedValueOnce({ info: info("bg"), messages: [], running: true });
    ipc.sessionMessages.mockResolvedValueOnce([
      { role: "user", content: "long task" },
      { role: "assistant", content: "step one done" },
    ]);
    await useStore.getState().resume("bg");
    expect(ipc.sessionMessages).toHaveBeenCalledWith("bg");
    const kinds = useStore.getState().threads.bg.map((it) => it.kind);
    expect(kinds).toEqual(["user", "assistant", "assistant"]);
    expect(useStore.getState().threads.bg[2]).toMatchObject({ streaming: true, partial: true });

    // The rest of the reply streams into the reopened bubble.
    useStore.getState().ingestToken("bg", " and two");
    useStore.getState().flushTokens();
    expect(useStore.getState().threads.bg[2]).toMatchObject({ text: " and two" });
  });

  it("a running chat that still has its thread is never re-read from the transcript", async () => {
    useStore.setState({
      session: info("cur"),
      infos: { cur: info("cur"), bg: info("bg") },
      threads: { cur: [], bg: bubble("bg") },
      runStatus: { bg: "running" },
    });
    ipc.resumeSession.mockResolvedValueOnce({ info: info("bg"), messages: [], running: true });
    await useStore.getState().resume("bg");
    expect(ipc.sessionMessages).not.toHaveBeenCalled();
    expect(useStore.getState().threads.bg).toHaveLength(1);
  });

  it("a session switch trims staged snippets and preview errors along with threads", async () => {
    useStore.setState({
      session: info("cur"),
      infos: { cur: info("cur"), old: info("old") },
      threads: { old: [], a: [], b: [], c: [], cur: [] },
      snippets: { old: [{ path: "x.ts", start: 1, end: 2, code: "let x" }], cur: [] },
      previewErrors: { old: "boom", cur: undefined },
    });
    await useStore.getState().startNewSession();
    expect(useStore.getState().threads.old).toBeUndefined();
    expect(useStore.getState().snippets.old).toBeUndefined();
    expect(useStore.getState().previewErrors).not.toHaveProperty("old");
  });
});

describe("retainFleets", () => {
  it("keeps a fleet as long as the chat it runs in is cached or running", async () => {
    const { retainFleets, fleetsFor } = await import("./store");
    const lane = { name: "a", id: "l", status: "running" as const, activity: "", tail: "", tokens: 0 };
    const fleets = {
      "fleet-1": { session: "kept", source: "turn" as const, focused: null, lanes: [lane] },
      "fleet-2": { session: "gone", source: "turn" as const, focused: null, lanes: [lane] },
      "fleet-3": { session: "kept", source: "review" as const, focused: null, lanes: [lane] },
    };
    // Keyed by fleet id, so a sweep keyed by session id would drop every one;
    // the fleet's own session is what decides.
    const kept = retainFleets(fleets, new Set(["kept"]));
    expect(Object.keys(kept)).toEqual(["fleet-1", "fleet-3"]);
    expect(fleetsFor(kept, "kept").map(([id]) => id)).toEqual(["fleet-1", "fleet-3"]);
    expect(fleetsFor(kept, "gone")).toEqual([]);
  });
});

describe("capThreadSessions", () => {
  it("ranks running sessions by activity, then by recency of insertion", async () => {
    const { capThreadSessions } = await import("./store");
    const threads = Object.fromEntries(
      ["a", "b", "c", "d", "e", "f", "g", "h"].map((id) => [id, [] as import("../features/chat/thread").Item[]]),
    );
    const runStatus = Object.fromEntries(Object.keys(threads).map((id) => [id, "running" as const]));
    // "a" is oldest but most active; with no other activity the newest six
    // by insertion fill the remaining protected slots.
    const kept = Object.keys(capThreadSessions(threads, runStatus, "cur", new Map([["a", 10]])));
    expect(kept).toEqual(["a", "d", "e", "f", "g", "h"]);
  });
});

describe("store: chat tabs", () => {
  const W = ipc.sampleSession.workspace;
  const info = (id: string) => ({ ...ipc.sampleSession, session_id: id });
  const summary = (id: string) => ({
    id,
    workspace: W,
    model: "m",
    created_at: 1,
    title: id,
    message_count: 2,
    review_status: "" as const,
    source: "",
  });
  /** Make `resume(id)` answer with a cold transcript for that id. */
  const coldResume = () =>
    ipc.resumeSession.mockImplementation(async (id = "") => ({
      info: info(id),
      messages: [],
      running: false,
    }));
  const tabs = () => useStore.getState().chatTabs[W];

  it("a new chat opens as a tab in its project's strip, persisted", async () => {
    await useStore.getState().startNewSession();
    expect(tabs()).toEqual(["new-session-id"]);
    expect(getUi("chatTabs")).toEqual({ [W]: ["new-session-id"] });
  });

  it("opening a chat appends its tab once; reopening keeps the order", async () => {
    coldResume();
    useStore.setState({ session: info("a"), chatTabs: { [W]: ["a"] } });
    await useStore.getState().resume("b");
    await useStore.getState().resume("c");
    await useStore.getState().resume("b");
    expect(tabs()).toEqual(["a", "b", "c"]);
    expect(useStore.getState().session?.session_id).toBe("b");
  });

  it("closing a background tab leaves the visible chat alone", async () => {
    useStore.setState({ session: info("a"), chatTabs: { [W]: ["a", "b"] } });
    await useStore.getState().closeTab("b");
    expect(tabs()).toEqual(["a"]);
    expect(ipc.resumeSession).not.toHaveBeenCalled();
    expect(useStore.getState().session?.session_id).toBe("a");
  });

  it("closing the visible chat lands on its right neighbour, else its left", async () => {
    coldResume();
    useStore.setState({ session: info("b"), chatTabs: { [W]: ["a", "b", "c"] } });
    await useStore.getState().closeTab("b");
    expect(tabs()).toEqual(["a", "c"]);
    expect(useStore.getState().session?.session_id).toBe("c");

    await useStore.getState().closeTab("c");
    expect(tabs()).toEqual(["a"]);
    expect(useStore.getState().session?.session_id).toBe("a");
  });

  it("closing the last tab opens a fresh chat — unless it already is one", async () => {
    // "a" has history, so closing it leaves nothing to show: mint a new chat.
    useStore.setState({ session: info("a"), sessions: [summary("a")], chatTabs: { [W]: ["a"] } });
    await useStore.getState().closeTab("a");
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(tabs()).toEqual(["new-session-id"]);

    // The fresh chat is the only tab and untouched: closing it stays put
    // rather than minting orphans.
    await useStore.getState().closeTab("new-session-id");
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(tabs()).toEqual(["new-session-id"]);
  });

  it("close others keeps one tab and shows it", async () => {
    coldResume();
    useStore.setState({ session: info("a"), chatTabs: { [W]: ["a", "b", "c"] } });
    await useStore.getState().closeOtherTabs("c");
    expect(tabs()).toEqual(["c"]);
    expect(useStore.getState().session?.session_id).toBe("c");
  });

  it("close to the right drops the tabs after it, showing it if the visible chat went", async () => {
    coldResume();
    useStore.setState({ session: info("a"), chatTabs: { [W]: ["a", "b", "c", "d"] } });
    await useStore.getState().closeTabsRight("c");
    expect(tabs()).toEqual(["a", "b", "c"]);
    expect(useStore.getState().session?.session_id).toBe("a"); // untouched

    await useStore.getState().closeTabsRight("b");
    expect(tabs()).toEqual(["a", "b"]);
    expect(useStore.getState().session?.session_id).toBe("a");

    useStore.setState({ session: info("b") });
    await useStore.getState().closeTabsRight("a");
    expect(tabs()).toEqual(["a"]);
    expect(useStore.getState().session?.session_id).toBe("a");
  });

  it("deleting chats drops their tabs; deleting the visible one lands on a survivor", async () => {
    coldResume();
    useStore.setState({ session: info("b"), chatTabs: { [W]: ["a", "b", "c"] } });
    await useStore.getState().removeSessions(["b", "c"]);
    expect(tabs()).toEqual(["a"]);
    expect(useStore.getState().session?.session_id).toBe("a");
    expect(ipc.newSession).not.toHaveBeenCalled();
  });

  it("pruning sheds tabs the history doesn't know, keeping the visible chat", () => {
    useStore.setState({
      session: info("fresh"),
      sessions: [summary("a")],
      chatTabs: { [W]: ["a", "gone", "fresh"], "/other": ["stale"] },
    });
    useStore.getState().pruneTabs();
    expect(useStore.getState().chatTabs).toEqual({ [W]: ["a", "fresh"] });
    expect(getUi("chatTabs")).toEqual({ [W]: ["a", "fresh"] });
  });
});

describe("store: renaming a chat", () => {
  it("patches the listed title at once, writes it, then re-reads the list and board", async () => {
    useStore.setState({
      sessions: [{ id: "a", workspace: "/w", model: "m", created_at: 1, title: "first words", message_count: 2, review_status: "", source: "" }],
    });
    await useStore.getState().renameSession("a", "  Parser flake ");
    expect(ipc.renameSession).toHaveBeenCalledWith("a", "Parser flake");
    expect(ipc.listSessions).toHaveBeenCalled();
    expect(ipc.ledgerSnapshot).toHaveBeenCalled();
  });

  it("a blank name clears the custom title without touching the list", async () => {
    useStore.setState({
      sessions: [{ id: "a", workspace: "/w", model: "m", created_at: 1, title: "first words", message_count: 2, review_status: "", source: "" }],
    });
    ipc.listSessions.mockResolvedValueOnce([] as never);
    await useStore.getState().renameSession("a", "   ");
    expect(ipc.renameSession).toHaveBeenCalledWith("a", "");
  });
});

describe("store: chat tabs, hardened", () => {
  const W = ipc.sampleSession.workspace;
  const info = (id: string) => ({ ...ipc.sampleSession, session_id: id });

  it("moveTab reorders within the strip and persists; a no-op move writes nothing", () => {
    useStore.setState({ chatTabs: { [W]: ["a", "b", "c"] } });
    useStore.getState().moveTab("c", "a");
    expect(useStore.getState().chatTabs[W]).toEqual(["c", "a", "b"]);
    expect(getUi("chatTabs")).toEqual({ [W]: ["c", "a", "b"] });
    const before = useStore.getState().chatTabs;
    useStore.getState().moveTab("c", "a");
    expect(useStore.getState().chatTabs).toBe(before);
  });

  it("removing a project drops its strip", async () => {
    useStore.setState({ chatTabs: { [W]: ["a"], "/gone": ["x", "y"] } });
    await useStore.getState().removeProject("/gone");
    expect(useStore.getState().chatTabs).toEqual({ [W]: ["a"] });
  });

  it("closing the visible chat falls back to a fresh chat when its neighbour won't open", async () => {
    ipc.resumeSession.mockRejectedValueOnce(new Error("no such session"));
    useStore.setState({ session: info("a"), chatTabs: { [W]: ["a", "b"] } });
    await useStore.getState().closeTab("a");
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(useStore.getState().session?.session_id).toBe("new-session-id");
    // The dead neighbour is gone from the strip too.
    expect(useStore.getState().chatTabs[W]).toEqual(["new-session-id"]);
  });

  it("refreshHistory says whether the list loaded, keeping the old one when it didn't", async () => {
    useStore.setState({ sessions: [{ id: "keep", workspace: W, model: "m", created_at: 1, title: "t", message_count: 1, review_status: "", source: "" }] });
    ipc.listSessions.mockRejectedValueOnce(new Error("backend away"));
    expect(await useStore.getState().refreshHistory()).toBe(false);
    expect(useStore.getState().sessions.map((s) => s.id)).toEqual(["keep"]);
    expect(await useStore.getState().refreshHistory()).toBe(true);
  });
});

describe("store: entering a project opens its loose ends as tabs", () => {
  const now = Math.floor(Date.now() / 1000);
  // The fresh chat a project opens with roots in that project.
  beforeEach(() => {
    ipc.newSession.mockImplementation(async () => ({ ...ipc.sampleSession, session_id: "new-session-id", workspace: "/w" }));
  });
  const entry = (id: string, workspace: string, extra: Partial<LedgerEntry> = {}): LedgerEntry => ({
    id,
    workspace,
    model: "m",
    created_at: now - 86_400,
    last_activity_at: now - 3_600,
    title: id,
    last_reply: "",
    message_count: 2,
    mid_turn: false,
    plan: null,
    trail: null,
    settle: null,
    review_status: "",
    seen_at: 0,
    ...extra,
  });

  it("opens open threads (needs you first, then newest), skipping settled and archived ones", async () => {
    ipc.ledgerSnapshot.mockResolvedValue({
      entries: [
        entry("older", "/w", { last_activity_at: now - 7_200 }),
        entry("newer", "/w"),
        // The reply never arrived: needs the user, so it leads.
        entry("dangling", "/w", { mid_turn: true, last_activity_at: now - 10_000 }),
        entry("tied", "/w", { settle: { settled_at: now - 60, note: "" } }),
        entry("archived", "/w", { last_activity_at: now - 20 * 86_400 }),
        entry("elsewhere", "/other"),
      ],
      running: [],
      last_seen: now,
    });
    await useStore.getState().prepareProject("/w");
    expect(useStore.getState().chatTabs["/w"]).toEqual(["dangling", "newer", "older", "new-session-id"]);
    expect(useStore.getState().chatTabs["/other"]).toBeUndefined();
  });

  it("keeps tabs already open where they were, adding only what's missing", async () => {
    ipc.ledgerSnapshot.mockResolvedValue({
      entries: [entry("a", "/w"), entry("b", "/w", { last_activity_at: now - 7_200 })],
      running: [],
      last_seen: now,
    });
    useStore.setState({ chatTabs: { "/w": ["b", "mine"] } });
    await useStore.getState().prepareProject("/w");
    expect(useStore.getState().chatTabs["/w"]).toEqual(["b", "mine", "a", "new-session-id"]);
  });

  it("a project with nothing on the trail just gets the fresh chat", async () => {
    ipc.newSession.mockImplementation(async () => ({ ...ipc.sampleSession, session_id: "new-session-id", workspace: "/quiet" }));
    await useStore.getState().prepareProject("/quiet");
    expect(useStore.getState().chatTabs["/quiet"]).toEqual(["new-session-id"]);
  });
});
