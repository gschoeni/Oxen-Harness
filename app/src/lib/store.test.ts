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
      info: { model: "", workspace: "", session_id: "bg" },
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

  it("resume is a no-op when the target is already the active session", async () => {
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "same" } });
    await useStore.getState().resume("same");
    expect(ipc.resumeSession).not.toHaveBeenCalled();
  });
});
