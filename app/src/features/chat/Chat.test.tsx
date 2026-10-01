import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Chat } from "./Chat";
import { TitleBar } from "../../TitleBar";
import { useStore } from "../../lib/store";
import { startTurn, transcriptToItems } from "./thread";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

// Streaming is wired at the app level into the store, so tests drive the store's
// ingest actions directly (as App's event subscription would).
const token = (session: string, t: string) =>
  act(() => useStore.getState().ingestToken(session, t));
const tool = (e: { session: string; phase: "start" | "end"; name: string; detail: string }) =>
  act(() => useStore.getState().ingestTool(e));

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
    threads: { s1: [] },
  });
});

/** Simulate a file dropped from the OS: a `File` in `dataTransfer.files`. */
function dropOsFile(target: HTMLElement, name: string) {
  const file = new File([new Uint8Array([137, 80, 78, 71])], name, { type: "application/octet-stream" });
  const dataTransfer = { types: ["Files"], files: [file] as unknown as FileList, getData: () => "", dropEffect: "none" };
  fireEvent.dragOver(target, { dataTransfer });
  fireEvent.drop(target, { dataTransfer });
}

describe("Chat", () => {
  it("opens the active project's files and settings from the titlebar", async () => {
    useStore.setState({
      homeOpen: false,
      projects: [{
        path: ipc.sampleSession.workspace,
        name: "Demo",
        description: "",
        instructions: "",
        context: [],
        remote_repo: null,        session_count: 1,
        active: true,
        last_used_at: null,
      }],
    });
    // The project-home button lives in the app title bar, beside the chat.
    render(
      <>
        <TitleBar />
        <Chat />
      </>,
    );

    await userEvent.click(screen.getByRole("button", { name: "Project files and settings" }));

    expect(useStore.getState().homeOpen).toBe(true);
    expect(useStore.getState().projectHomePath).toBe(ipc.sampleSession.workspace);
  });

  it("lists desktop slash commands on slash and omits exit", async () => {
    render(<Chat />);
    const box = screen.getByPlaceholderText(/ask the agent/i);
    await userEvent.type(box, "/");
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /\/queue/i })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /\/loop/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /\/exit/i })).not.toBeInTheDocument();
  });

  it("executes recognized commands locally and leaves unknown slash prompts for the model", async () => {
    render(<Chat />);
    const box = screen.getByPlaceholderText(/ask the agent/i);
    await userEvent.type(box, "/usage");
    await userEvent.keyboard("{Enter}");
    expect(useStore.getState().settingsPage).toBe("usage");
    expect(ipc.runTurn).not.toHaveBeenCalled();

    useStore.getState().setSettingsOpen(false);
    await userEvent.type(box, "/frobnicate this");
    await userEvent.keyboard("{Enter}");
    expect(ipc.runTurn).toHaveBeenCalledWith("s1", "/frobnicate this", []);
  });

  it("shows the empty state with the hero game attract screen and example prompts", () => {
    render(<Chat />);
    expect(screen.getByText("OXEN TRAIL")).toBeInTheDocument();
    // The game waits on its attract screen for the ↑ ↑ ↓ ↓ start combo.
    expect(screen.getByLabelText(/Trail of Understanding\. Press up, up, down, down to play/i)).toBeInTheDocument();
    expect(screen.queryByText(/Send a message to begin on your trail/i)).not.toBeInTheDocument();
    expect(screen.getByText("Explain this codebase")).toBeInTheDocument();
  });

  it("keeps the hero up when a fresh chat holds only system notices", async () => {
    render(<Chat />);
    // `/location` (and other local commands) drop a notice into an otherwise
    // empty thread; that must not replace the hero with a lone faint line.
    act(() => useStore.getState().addNotice("Location set to Southern Oregon."));
    expect(screen.getByText("Location set to Southern Oregon.")).toBeInTheDocument();
    expect(screen.getByText("OXEN TRAIL")).toBeInTheDocument();
    expect(screen.getByText("Explain this codebase")).toBeInTheDocument();
    // The first real message starts the conversation view.
    act(() => {
      const s = useStore.getState();
      useStore.setState({ threads: { s1: startTurn(s.threads.s1 ?? [], "howdy") } });
    });
    expect(screen.queryByText("OXEN TRAIL")).not.toBeInTheDocument();
    expect(screen.getByText("Location set to Southern Oregon.")).toBeInTheDocument();
  });

  it("starts the hero game only after the full start combo", async () => {
    render(<Chat />);
    // Arrow keys aimed at the composer never reach the game — step away first.
    (document.activeElement as HTMLElement | null)?.blur();
    // A partial or broken sequence leaves the attract screen up.
    await userEvent.keyboard("{ArrowUp}{ArrowDown}");
    expect(screen.getByLabelText(/Press up, up, down, down to play/i)).toBeInTheDocument();
    // The full ↑ ↑ ↓ ↓ combo starts the run.
    await userEvent.keyboard("{ArrowUp}{ArrowUp}{ArrowDown}{ArrowDown}");
    expect(screen.getByLabelText(/Trail of Understanding\. Press escape to make camp/i)).toBeInTheDocument();
    // Escape makes camp: back to the attract screen.
    await userEvent.keyboard("{Escape}");
    expect(screen.getByLabelText(/Press up, up, down, down to play/i)).toBeInTheDocument();
  });

  it("switches the hero cabinet between games from the attract screen", async () => {
    render(<Chat />);
    // Both cabinets are offered as tabs; picking one swaps the attract screen.
    expect(screen.getByLabelText(/Trail of Understanding\. Press up, up, down, down/i)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Trail" }));
    expect(screen.getByLabelText(/The Oxen Trail\. Press up, up, down, down/i)).toBeInTheDocument();
    // …and the pick is remembered over the default.
    expect(useStore.getState().heroGame).toBe("oregon");
  });

  it("opens the arcade dock during a run so you can play while streaming", async () => {
    ipc.runTurn.mockImplementationOnce(() => new Promise(() => {})); // stays in flight
    render(
      <>
        <TitleBar />
        <Chat />
      </>,
    );
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "go");
    await userEvent.keyboard("{Enter}");
    // With a turn in flight the title bar offers the arcade toggle.
    await userEvent.click(screen.getByLabelText(/Toggle the arcade/i));
    expect(screen.getByRole("dialog", { name: /arcade/i })).toBeInTheDocument();
    // The dock hosts the same cabinet, playable without leaving the chat.
    expect(screen.getByRole("tab", { name: "Trail" })).toBeInTheDocument();
  });

  it("sends a typed message and renders the user + assistant turn", async () => {
    render(<Chat />);
    const box = screen.getByPlaceholderText(/ask the agent/i);
    await userEvent.type(box, "build it");
    await userEvent.keyboard("{Enter}");

    expect(screen.getByText("build it")).toBeInTheDocument();
    expect(ipc.runTurn).toHaveBeenCalledWith("s1", "build it", []);
    expect(await screen.findByText("Done.")).toBeInTheDocument();
  });

  it("runs an example prompt when its chip is clicked", async () => {
    render(<Chat />);
    await userEvent.click(screen.getByText("Summarize recent git changes"));
    expect(ipc.runTurn).toHaveBeenCalledWith("s1", "Summarize recent git changes", []);
  });

  it("streams tokens into the in-flight assistant bubble", async () => {
    let resolveTurn!: (v: string) => void;
    ipc.runTurn.mockImplementationOnce(() => new Promise((r) => (resolveTurn = r)));

    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "hi");
    await userEvent.keyboard("{Enter}");

    token("s1", "Hel");
    token("s1", "lo!");
    expect(await screen.findByText("Hello!")).toBeInTheDocument();

    act(() => resolveTurn(""));
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());
  });

  it("renders tool activity as running then done", async () => {
    let resolveTurn!: (v: string) => void;
    ipc.runTurn.mockImplementationOnce(() => new Promise((r) => (resolveTurn = r)));

    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "go");
    await userEvent.keyboard("{Enter}");

    tool({ session: "s1", phase: "start", name: "run_shell", detail: '{"command":"ls"}' });
    // The tool card shows a human summary: the verb and the command.
    expect(await screen.findByText("Ran")).toBeInTheDocument();
    expect(screen.getByText("ls")).toBeInTheDocument();

    tool({ session: "s1", phase: "end", name: "run_shell", detail: "a.ts b.ts" });
    act(() => resolveTurn("finished"));
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());

    // Expanding the finished card reveals the tool's output.
    await userEvent.click(screen.getByText("Ran"));
    expect(await screen.findByText(/a\.ts b\.ts/)).toBeInTheDocument();
  });

  it("shows a stop button mid-turn and cancels the run when clicked", async () => {
    let resolveTurn!: (v: string) => void;
    ipc.runTurn.mockImplementationOnce(() => new Promise((r) => (resolveTurn = r)));

    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "go");
    await userEvent.keyboard("{Enter}");

    // While running, the send button is replaced by a stop button.
    const stopBtn = await screen.findByRole("button", { name: /stop generating/i });
    await userEvent.click(stopBtn);
    expect(ipc.cancelTurn).toHaveBeenCalledWith("s1");

    // Resolving the (cancelled) turn settles the run status back to idle.
    act(() => resolveTurn(""));
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());
  });

  it("queues a message typed while the chat is mid-turn", async () => {
    useStore.setState({ runStatus: { s1: "running" } });
    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/queue a message/i), "later");
    await userEvent.keyboard("{Enter}");
    expect(screen.getByText("later")).toBeInTheDocument();
    expect(screen.getByText(/Queued · 1/)).toBeInTheDocument();
    expect(ipc.runTurn).not.toHaveBeenCalled();
  });

  it("attaches dropped files and sends them with the prompt", async () => {
    const { container } = render(<Chat />);
    ipc.stageDroppedFile.mockResolvedValueOnce("/Users/dev/Desktop/diagram.png");
    dropOsFile(container.querySelector("form.composer") as HTMLElement, "diagram.png");
    expect(await screen.findByLabelText("Remove diagram.png")).toBeInTheDocument();

    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "what is this");
    await userEvent.keyboard("{Enter}");
    expect(ipc.runTurn).toHaveBeenCalledWith("s1", "what is this", [
      "/Users/dev/Desktop/diagram.png",
    ]);
    // The tray clears once the message is sent.
    expect(screen.queryByLabelText("Remove diagram.png")).toBeNull();
  });

  it("accepts an in-app image drag dropped on the composer and lights it while hovering", async () => {
    const { container } = render(<Chat />);
    const main = container.querySelector("main.chat") as HTMLElement;
    const composer = container.querySelector("form.composer") as HTMLElement;
    const mime = "application/x-oxen-workspace-files";
    const paths = JSON.stringify(["/w/generations/2026-09-14/0413-ox-1.png"]);
    const dt = {
      types: [mime],
      getData: (t: string) => (t === mime ? paths : ""),
      dropEffect: "none",
    };
    fireEvent.dragEnter(main, { dataTransfer: dt });
    expect(main.classList.contains("chat-drop-active")).toBe(true);
    fireEvent.dragOver(composer, { dataTransfer: dt });
    fireEvent.drop(composer, { dataTransfer: dt });
    expect(await screen.findByText(/0413-ox-1\.png/)).toBeInTheDocument();
    expect(main.classList.contains("chat-drop-active")).toBe(false);
    // The file lands in the prompt bar's own media tray, counted by kind.
    const tray = screen.getByRole("list", { name: /attached media/i });
    expect(tray.closest("form.composer")).toBe(composer);
    expect(screen.getByText("1 image")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /remove 0413-ox-1\.png/i })).toBeInTheDocument();
  });

  it("removes a pending attachment when its ✕ is clicked", async () => {
    render(<Chat />);
    ipc.stageDroppedFile.mockResolvedValueOnce("/tmp/a.pdf");
    dropOsFile(document.querySelector("form.composer") as HTMLElement, "a.pdf");
    await userEvent.click(await screen.findByRole("button", { name: /remove a\.pdf/i }));
    expect(screen.queryByText(/a\.pdf/)).toBeNull();
  });

  it("offers an inline API-key form on a 401, then saves the key and retries the turn", async () => {
    // The first attempt fails auth; the retry (after the key is saved) succeeds.
    ipc.runTurn.mockRejectedValueOnce("Oxen API error (401): You must be authenticated");
    ipc.retryTurn.mockResolvedValueOnce("Here is your README.");

    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "Write me a README");
    await userEvent.keyboard("{Enter}");

    // Instead of a dead-end error, the auth prompt appears with the user's turn intact.
    expect(await screen.findByText(/Connect your Oxen account/i)).toBeInTheDocument();
    expect(screen.getByText("Write me a README")).toBeInTheDocument();
    // The card names the host the key will authenticate against.
    expect(await screen.findByText("hub.oxen.ai")).toBeInTheDocument();
    // The turn settled to idle so the composer isn't stuck "busy".
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());

    await userEvent.type(screen.getByPlaceholderText(/Oxen API key/i), "sk-live-key");
    await userEvent.click(screen.getByRole("button", { name: /save & retry/i }));

    expect(ipc.configureOxenKey).toHaveBeenCalledWith("s1", "sk-live-key");
    expect(ipc.retryTurn).toHaveBeenCalledWith("s1");
    // The retried reply streams into the same chat; the key card is gone.
    expect(await screen.findByText("Here is your README.")).toBeInTheDocument();
    expect(screen.queryByText(/Connect your Oxen account/i)).toBeNull();
  });

  it("offers a retry card on a 402, then continues the chat once credits are added", async () => {
    // The first attempt runs out of credits; the retry (after topping up) succeeds.
    ipc.runTurn.mockRejectedValueOnce("Oxen API error (402): You have run out of credits.");
    ipc.retryTurn.mockResolvedValueOnce("Here is your README.");

    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "Write me a README");
    await userEvent.keyboard("{Enter}");

    // Instead of a dead-end error, the credits card appears with the turn intact.
    expect(await screen.findByText(/Out of Oxen credits/i)).toBeInTheDocument();
    expect(screen.getByText("Write me a README")).toBeInTheDocument();
    // It links to the connected hub to add credits.
    const link = screen.getByRole("link", { name: /add credits/i });
    expect(link).toHaveAttribute("href", "https://hub.oxen.ai/settings");
    // The turn settled to idle so the composer isn't stuck "busy".
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());

    await userEvent.click(screen.getByRole("button", { name: /retry/i }));
    expect(ipc.retryTurn).toHaveBeenCalledWith("s1");
    // The retried reply streams into the same chat; the card is gone.
    expect(await screen.findByText("Here is your README.")).toBeInTheDocument();
    expect(screen.queryByText(/Out of Oxen credits/i)).toBeNull();
  });

  it("offers a retry card when the provider stays down, then continues after e.g. a model switch", async () => {
    // The backend already retried with backoff; this is the surfaced failure.
    ipc.runTurn.mockRejectedValueOnce(
      "the model endpoint failed 4 times in a row (gpt-5-5 at https://hub.oxen.ai/api/ai) — " +
        "last error: Oxen API error (502): The model provider returned an error.",
    );
    ipc.retryTurn.mockResolvedValueOnce("Back on the trail.");

    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "Write me a README");
    await userEvent.keyboard("{Enter}");

    // Instead of a dead-end error bubble, the continue card appears carrying
    // the full diagnostic (attempts, model, endpoint, provider error).
    expect(await screen.findByText(/Continue this chat/i)).toBeInTheDocument();
    expect(screen.getByText(/failed 4 times in a row/)).toBeInTheDocument();
    expect(screen.getByText(/502/)).toBeInTheDocument();
    // The turn settled to idle so the composer isn't stuck "busy".
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());

    // One click re-drives the same turn (works after switching models too,
    // since the swap keeps the session).
    await userEvent.click(screen.getByRole("button", { name: /continue/i }));
    expect(ipc.retryTurn).toHaveBeenCalledWith("s1");
    expect(await screen.findByText("Back on the trail.")).toBeInTheDocument();
    expect(screen.queryByText(/Continue this chat/i)).toBeNull();
  });

  it("drops the retry card when a fresh prompt supersedes it", async () => {
    ipc.runTurn.mockRejectedValueOnce("Oxen API error (402): You have run out of credits.");
    render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "first try");
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByText(/Out of Oxen credits/i)).toBeInTheDocument();
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());

    // Typing a new message instead of retrying replaces the card with a fresh turn.
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "try something else");
    await userEvent.keyboard("{Enter}");
    expect(screen.queryByText(/Out of Oxen credits/i)).toBeNull();
    expect(await screen.findByText("Done.")).toBeInTheDocument();
  });

  it("offers to continue a resumed chat whose transcript stopped mid-turn", async () => {
    ipc.resumeSession.mockResolvedValueOnce({
      info: { ...ipc.sampleSession, session_id: "broken" },
      messages: [
        { role: "user", content: "old question" },
        { role: "assistant", content: "old answer" },
        { role: "user", content: "the one that died" },
      ],
      running: false,
    });
    ipc.retryTurn.mockResolvedValueOnce("Picking up where we left off.");

    render(<Chat />);
    await act(() => useStore.getState().resume("broken"));

    // The transcript renders with a continue card in place of the missing reply.
    expect(screen.getByText("the one that died")).toBeInTheDocument();
    expect(screen.getByText(/stopped before the reply finished/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /continue/i }));
    expect(ipc.retryTurn).toHaveBeenCalledWith("broken");
    expect(await screen.findByText("Picking up where we left off.")).toBeInTheDocument();
    expect(screen.queryByText(/stopped before the reply finished/i)).toBeNull();
  });

  it("stops the plan card spinner once the run ends", async () => {
    let resolveTurn!: (v: string) => void;
    ipc.runTurn.mockImplementationOnce(() => new Promise((r) => (resolveTurn = r)));

    const { container } = render(<Chat />);
    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "big task");
    await userEvent.keyboard("{Enter}");

    const args = JSON.stringify({
      plan: [
        { content: "Research", active_form: "Researching", status: "in_progress" },
        { content: "Build", active_form: "Building", status: "pending" },
      ],
    });
    tool({ session: "s1", phase: "start", name: "update_plan", detail: args });
    tool({ session: "s1", phase: "end", name: "update_plan", detail: "Plan (0/2 done)" });

    // While the run is live: the active step spins and nothing reads stalled.
    expect((await screen.findAllByText("Researching")).length).toBeGreaterThan(0);
    expect(container.querySelector(".plan-spinner")).not.toBeNull();

    // The turn ends (the model gave up after a failed step) with items open:
    // the spinner stops.
    act(() => resolveTurn("stopping here"));
    await waitFor(() => expect(useStore.getState().runStatus["s1"]).toBeUndefined());
    expect(container.querySelector(".plan-spinner")).toBeNull();
  });

  it("renders a resumed transcript from the store", () => {
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "x" },
      threads: {
        x: transcriptToItems([
          { role: "user", content: "previous question" },
          { role: "assistant", content: "previous answer" },
        ]),
      },
    });
    render(<Chat />);
    expect(screen.getByText("previous question")).toBeInTheDocument();
    expect(screen.getByText("previous answer")).toBeInTheDocument();
  });
});


describe("chat scroll following", () => {
  const append = (text: string) => act(() => useStore.setState((s) => ({
    threads: { ...s.threads, s1: [{ id: "stream", kind: "assistant", text, streaming: true }] },
  })));
  const resizeCallbacks = new Set<ResizeObserverCallback>();

  beforeEach(() => {
    resizeCallbacks.clear();
    vi.stubGlobal("ResizeObserver", class {
      constructor(private callback: ResizeObserverCallback) { resizeCallbacks.add(callback); }
      observe() {}
      unobserve() {}
      disconnect() { resizeCallbacks.delete(this.callback); }
    });
  });

  afterEach(() => vi.unstubAllGlobals());

  function setupScroll() {
    useStore.setState({ runStatus: { s1: "running" } });
    append("An answer");
    const { container, unmount } = render(<Chat />);
    const viewport = container.querySelector<HTMLElement>(".messages")!;
    const size = { height: 1200, viewport: 400, top: 800 };
    Object.defineProperties(viewport, {
      scrollHeight: { get: () => size.height },
      clientHeight: { get: () => size.viewport },
      scrollTop: {
        get: () => size.top,
        set: (value: number) => { size.top = Math.max(0, Math.min(value, size.height - size.viewport)); },
      },
    });
    const resize = () => act(() => {
      for (const callback of resizeCallbacks) callback([], {} as ResizeObserver);
    });
    append(" more");
    fireEvent.scroll(viewport);
    return { viewport, size, resize, unmount };
  }

  it("keeps following after layout growth emits a scroll event", () => {
    const { viewport, size, resize } = setupScroll();
    size.height += 500;
    fireEvent.scroll(viewport);
    resize();
    expect(size.top).toBe(1300);
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
  });

  it("follows independently resized content and a shrinking viewport", () => {
    const { size, resize } = setupScroll();
    size.height += 240; // e.g. an image finishes loading, without a new token
    resize();
    expect(size.top).toBe(1040);
    size.viewport -= 120; // e.g. the agents panel or composer expands
    resize();
    expect(size.top).toBe(1160);
  });

  it("lets even a small upward scroll pause streaming until latest is clicked", () => {
    const { viewport, size } = setupScroll();
    size.top -= 20;
    fireEvent.scroll(viewport);
    expect(screen.getByRole("button", { name: "Scroll to latest" })).toBeInTheDocument();
    size.height += 300;
    append(" still streaming");
    expect(size.top).toBe(780);
    fireEvent.click(screen.getByRole("button", { name: "Scroll to latest" }));
    expect(size.top).toBe(1100);
    size.height += 100;
    append(" and following again");
    expect(size.top).toBe(1200);
  });

  it("resumes following when manually scrolled back to the bottom", () => {
    const { viewport, size } = setupScroll();
    size.top = 400;
    fireEvent.scroll(viewport);
    size.top = 800;
    fireEvent.scroll(viewport);
    size.height += 100;
    append(" more");
    expect(size.top).toBe(900);
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
  });

  it("preserves the reading position across content and viewport resizes", () => {
    const { viewport, size, resize } = setupScroll();
    size.top = 400;
    fireEvent.scroll(viewport);
    size.height += 300;
    size.viewport -= 100;
    resize();
    append(" more");
    expect(size.top).toBe(400);
  });

  it("starts a newly selected chat at the bottom even if the old chat was paused", () => {
    const { viewport, size } = setupScroll();
    size.top = 400;
    fireEvent.scroll(viewport);
    size.height = 2000;
    act(() => useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s2" },
      threads: { s2: [{ id: "reply", kind: "assistant", text: "Another chat", streaming: false }] },
    }));
    expect(size.top).toBe(1600);
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
  });

  it("honors an upward wheel gesture arriving at the same time as new content", () => {
    const { viewport, size, resize } = setupScroll();
    fireEvent.wheel(viewport, { deltaY: -30 });
    size.height += 300;
    resize();
    append(" more");
    expect(size.top).toBe(800);
    expect(screen.getByRole("button", { name: "Scroll to latest" })).toBeInTheDocument();
  });

  it("keeps following when an upward wheel scroll belongs to a nested code block", () => {
    const { viewport, size } = setupScroll();
    const code = document.createElement("pre");
    code.style.overflowY = "auto";
    code.scrollTop = 80;
    viewport.append(code);
    fireEvent.wheel(code, { deltaY: -20 });
    size.height += 100;
    append(" more");
    expect(size.top).toBe(900);
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
    code.remove();
  });

  it("follows a content collapse instead of treating its scroll clamp as an upward gesture", () => {
    const { viewport, size, resize } = setupScroll();
    size.height -= 300;
    size.top = 500;
    fireEvent.scroll(viewport);
    resize();
    size.height += 100;
    append(" more");
    expect(size.top).toBe(600);
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
  });

  it("returns to following when a prompt is queued while reading above", async () => {
    const { viewport, size } = setupScroll();
    size.top = 400;
    fireEvent.scroll(viewport);
    await userEvent.type(screen.getByPlaceholderText(/queue a message/i), "Continue");
    await userEvent.keyboard("{Enter}");
    expect(size.top).toBe(800);
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
  });

  it("can pause again after a content collapse brings the reader back to the bottom", () => {
    const { viewport, size, resize } = setupScroll();
    size.top = 400;
    fireEvent.scroll(viewport);
    size.height = 800;
    resize();
    expect(screen.queryByRole("button", { name: "Scroll to latest" })).not.toBeInTheDocument();
    size.top -= 20;
    fireEvent.scroll(viewport);
    size.height += 300;
    append(" more");
    expect(size.top).toBe(380);
    expect(screen.getByRole("button", { name: "Scroll to latest" })).toBeInTheDocument();
  });

  it("disconnects its layout observer when the chat unmounts", () => {
    const { unmount } = setupScroll();
    expect(resizeCallbacks.size).toBeGreaterThan(0);
    unmount();
    expect(resizeCallbacks.size).toBe(0);
  });
});

describe("chat: OS file drop", () => {
  it("stages a dropped File's bytes through the backend and attaches the returned path", async () => {
    const { container } = render(<Chat />);
    dropOsFile(container.querySelector("form.composer") as HTMLElement, "photo.png");
    await waitFor(() => expect(ipc.stageDroppedFile).toHaveBeenCalledWith("photo.png", expect.any(Uint8Array)));
    await waitFor(() => expect(screen.getByLabelText("Remove photo.png")).toBeInTheDocument());
  });
});
