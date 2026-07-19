import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Preview } from "./Preview";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const ready = (session = "s1") =>
  act(() =>
    useStore.getState().ingestPreviewStatus({
      session,
      phase: "ready",
      name: "dev",
      command: "npm run dev",
      url: "http://localhost:5173",
      port: 5173,
      message: null,
    }),
  );

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
    // resetAll boots on the Projects page, which counts as an overlay the
    // native webview must hide under — close it so the pane can attach.
    projectsOpen: false,
    previews: {},
    previewClosed: {},
    rightTab: {},
  });
});

describe("Preview store", () => {
  it("a ready server reopens a closed pane and focuses the preview tab", () => {
    act(() => useStore.getState().closePreview());
    expect(useStore.getState().previewClosed.s1).toBe(true);

    ready();
    const s = useStore.getState();
    expect(s.previews.s1?.url).toBe("http://localhost:5173");
    expect(s.previewClosed.s1).toBe(false);
    expect(s.rightTab.s1).toBe("preview");
  });

  it("a canvas doc takes the panel over from the preview", () => {
    ready();
    act(() =>
      useStore.getState().ingestCanvas({
        session: "s1",
        id: "report",
        title: "Report",
        format: "markdown",
        content: "# hi",
      }),
    );
    expect(useStore.getState().rightTab.s1).toBe("canvas");
  });
});

describe("Preview pane", () => {
  it("shows the URL and attaches the native webview while ready", async () => {
    ready();
    render(<Preview />);
    expect(screen.getByText("http://localhost:5173")).toBeInTheDocument();
    // The placeholder measured itself and asked the backend to glue the
    // native webview to it.
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalled());
    const [session, bounds] = ipc.previewAttach.mock.calls[0] as unknown as [
      string,
      { width: number },
    ];
    expect(session).toBe("s1");
    expect(bounds).toHaveProperty("width");
  });

  it("toolbar drives reload / pop-out / stop / close", async () => {
    ready();
    render(<Preview />);
    await userEvent.click(screen.getByRole("button", { name: "Reload preview" }));
    expect(ipc.previewReload).toHaveBeenCalledWith("s1");

    await userEvent.click(screen.getByRole("button", { name: "Open in browser" }));
    expect(ipc.previewOpenExternal).toHaveBeenCalledWith("s1");

    await userEvent.click(screen.getByRole("button", { name: "Stop server" }));
    expect(ipc.previewStop).toHaveBeenCalledWith("s1");

    await userEvent.click(screen.getByRole("button", { name: "Close preview" }));
    expect(useStore.getState().previewClosed.s1).toBe(true);
  });

  it("shows the starting state before the server is reachable", () => {
    act(() =>
      useStore.getState().ingestPreviewStatus({
        session: "s1",
        phase: "starting",
        name: "dev",
        command: "npm run dev",
        url: null,
        port: null,
        message: null,
      }),
    );
    render(<Preview />);
    expect(screen.getByText(/Starting dev server/)).toBeInTheDocument();
    expect(ipc.previewAttach).not.toHaveBeenCalled();
  });

  it("explains an error and suggests asking the chat to fix it", async () => {
    act(() =>
      useStore.getState().ingestPreviewStatus({
        session: "s1",
        phase: "error",
        name: "dev",
        command: "npm run dev",
        url: null,
        port: null,
        message: "server exited (exit status: 1)",
      }),
    );
    render(<Preview />);
    expect(screen.getByText("The server hit a problem")).toBeInTheDocument();
    expect(screen.getByText(/server exited/)).toBeInTheDocument();
    expect(screen.getByText(/ask the chat to fix/i)).toBeInTheDocument();

    // One-click restart, no agent turn involved.
    await userEvent.click(screen.getByRole("button", { name: "Restart server" }));
    expect(ipc.previewRestart).toHaveBeenCalledWith("s1");
  });

  it("shows the Fix it banner on a page error and sends a fix prompt", async () => {
    ready();
    render(<Preview />);
    act(() =>
      useStore.getState().ingestPreviewConsole({
        session: "s1",
        text: "TypeError: x is undefined (app.js:3)",
      }),
    );
    expect(screen.getByText(/Something broke in the app/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Fix it" }));
    // The banner clears and a fix prompt goes to the agent.
    expect(useStore.getState().previewErrors.s1).toBeUndefined();
    await vi.waitFor(() => expect(ipc.runTurn).toHaveBeenCalled());
    const prompt = (ipc.runTurn.mock.calls[0] as unknown as [string, string])[1];
    expect(prompt).toContain("TypeError: x is undefined");

    // A dismissed banner sends nothing.
    act(() =>
      useStore.getState().ingestPreviewConsole({ session: "s1", text: "again" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Dismiss error" }));
    expect(useStore.getState().previewErrors.s1).toBeUndefined();
    expect(ipc.runTurn).toHaveBeenCalledTimes(1);
  });

  it("detaches the native webview while an overlay is open", async () => {
    ready();
    render(<Preview />);
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalled());
    ipc.previewAttach.mockClear();

    act(() => useStore.getState().setSettingsOpen(true));
    await vi.waitFor(() => expect(ipc.previewDetach).toHaveBeenCalled());
    expect(screen.getByText(/Preview hidden while a panel is open/)).toBeInTheDocument();
  });
});

describe("Preview pane — hiding rules (the native view paints over all DOM)", () => {
  it("detaches while any modal is open, not just full-window overlays", async () => {
    ready();
    render(<Preview />);
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalled());
    ipc.previewDetach.mockClear();

    // A modal rendered anywhere (e.g. the sidebar's delete-chat confirm) must
    // hide the preview, or it would paint over the dialog.
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim";
    act(() => {
      document.body.appendChild(scrim);
    });
    await vi.waitFor(() => expect(ipc.previewDetach).toHaveBeenCalled());

    ipc.previewAttach.mockClear();
    act(() => scrim.remove());
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalled());
  });

  it("detaches while a popover menu is open (composer pickers)", async () => {
    ready();
    render(<Preview />);
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalled());
    ipc.previewDetach.mockClear();

    // The composer's dropdowns (model, compression, review) sit next to the
    // right dock, so their popovers would otherwise paint behind the webview.
    const menu = document.createElement("div");
    menu.className = "menu picker-menu";
    act(() => {
      document.body.appendChild(menu);
    });
    await vi.waitFor(() => expect(ipc.previewDetach).toHaveBeenCalled());

    ipc.previewAttach.mockClear();
    act(() => menu.remove());
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalled());
  });

  it("switching chats detaches, then attaches the newly viewed chat's server", async () => {
    ready("s1");
    const { rerender } = render(<Preview />);
    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalledWith("s1", expect.anything()));
    ipc.previewAttach.mockClear();
    ipc.previewDetach.mockClear();

    // Chat B has its own server on another port.
    ready("s2");
    act(() => {
      useStore.setState({ session: { ...ipc.sampleSession, session_id: "s2" } });
    });
    rerender(<Preview />);

    await vi.waitFor(() => expect(ipc.previewAttach).toHaveBeenCalledWith("s2", expect.anything()));
    // The old session's view is never left showing under the new chat.
    expect(ipc.previewDetach).toHaveBeenCalled();
    expect(ipc.previewAttach).not.toHaveBeenCalledWith("s1", expect.anything());
  });

  it("a chat with no dev server renders nothing and attaches nothing", () => {
    const { container } = render(<Preview />);
    expect(container).toBeEmptyDOMElement();
    expect(ipc.previewAttach).not.toHaveBeenCalled();
  });
});

describe("Preview panel arbitration", () => {
  it("a restart does not steal the panel from a canvas being written", () => {
    ready();
    act(() => useStore.getState().setCanvasWriting("s1", true));
    expect(useStore.getState().rightTab.s1).toBe("canvas");

    // The server restarts (auto-verify cycle): ready fires again, but the
    // document in flight keeps the panel.
    ready();
    expect(useStore.getState().rightTab.s1).toBe("canvas");
  });

  it("a restart does not reopen a pane the user closed", () => {
    ready();
    act(() => useStore.getState().closePreview());
    // A *repeat* ready (restart) must respect the user's close…
    ready();
    expect(useStore.getState().previewClosed.s1).toBe(true);
  });

  it("picking the Preview tab reopens a closed pane", () => {
    ready();
    act(() => useStore.getState().closePreview());
    act(() => useStore.getState().setRightTab("preview"));
    expect(useStore.getState().previewClosed.s1).toBe(false);
  });

  it("opening a canvas doc claims the panel from the preview", () => {
    ready();
    expect(useStore.getState().rightTab.s1).toBe("preview");
    act(() =>
      useStore.getState().openCanvasDoc({
        id: "report",
        title: "Report",
        format: "markdown",
        content: "# hi",
      }),
    );
    expect(useStore.getState().rightTab.s1).toBe("canvas");
  });

  it("a stale status sync cannot clobber a fresher event", async () => {
    // The backend answers with an old snapshot while a newer event lands.
    ipc.previewStatus.mockImplementation(async () => {
      ready(); // the live event wins the race
      return {
        phase: "starting",
        name: "dev",
        command: "npm run dev",
        url: null,
        port: null,
        message: null,
      } as never;
    });
    await act(() => useStore.getState().syncPreview("s1"));
    expect(useStore.getState().previews.s1?.phase).toBe("ready");
  });
});
