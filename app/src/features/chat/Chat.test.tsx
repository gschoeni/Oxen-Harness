import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Chat } from "./Chat";
import { useStore } from "../../lib/store";
import { transcriptToItems } from "./thread";
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

describe("Chat", () => {
  it("shows the empty state with example prompts", () => {
    render(<Chat />);
    expect(screen.getByText("OXEN TRAIL")).toBeInTheDocument();
    expect(screen.getByText(/size up the situation/i)).toBeInTheDocument();
    expect(screen.getByText("Explain this codebase")).toBeInTheDocument();
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
    render(<Chat />);
    act(() => ipc.emit("fileDrop", ["/Users/dev/Desktop/diagram.png"]));
    expect(await screen.findByText(/diagram\.png/)).toBeInTheDocument();

    await userEvent.type(screen.getByPlaceholderText(/ask the agent/i), "what is this");
    await userEvent.keyboard("{Enter}");
    expect(ipc.runTurn).toHaveBeenCalledWith("s1", "what is this", [
      "/Users/dev/Desktop/diagram.png",
    ]);
    // The chip clears once the message is sent.
    expect(screen.queryByText(/diagram\.png/)).toBeNull();
  });

  it("removes a pending attachment when its ✕ is clicked", async () => {
    render(<Chat />);
    act(() => ipc.emit("fileDrop", ["/tmp/a.pdf"]));
    await userEvent.click(await screen.findByRole("button", { name: /remove a\.pdf/i }));
    expect(screen.queryByText(/a\.pdf/)).toBeNull();
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
