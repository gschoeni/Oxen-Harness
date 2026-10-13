import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { emit, resumeSession, runTurn, setActiveProject, takeLaunchSurface, useLocalModel } from "../test/ipcMock";
import { resetAll } from "../test/utils";
import { startCliOpenBridge } from "./cliOpen";
import { useStore } from "./store";

beforeEach(resetAll);

describe("cli open bridge", () => {
  it("enters the forwarded project and leaves the home takeover", async () => {
    const stop = startCliOpenBridge();
    useStore.getState().setHomeOpen(true);

    emit("projectOpen", { path: "/work/demo", surface: null });

    await vi.waitFor(() => expect(setActiveProject).toHaveBeenCalledWith("/work/demo"));
    await vi.waitFor(() => expect(useStore.getState().homeOpen).toBe(false));
    stop();
  });

  it("opens the gallery dock when the launch asked for it", async () => {
    const stop = startCliOpenBridge();
    emit("projectOpen", { path: "/work/demo", surface: "gallery" });
    await vi.waitFor(() => {
      const s = useStore.getState();
      const id = s.session?.session_id;
      expect(id).toBeTruthy();
      expect(s.rightTab[id!]).toBe("gallery");
    });
    stop();
  });

  it("opens a settings page or resumes a session from the surface", async () => {
    const stop = startCliOpenBridge();
    emit("projectOpen", { path: "/work/demo", surface: "settings:media" });
    await vi.waitFor(() => {
      expect(useStore.getState().settingsOpen).toBe(true);
      expect(useStore.getState().settingsPage).toBe("media");
    });
    emit("projectOpen", { path: "/work/demo", surface: "session:abc123" });
    await vi.waitFor(() => expect(resumeSession).toHaveBeenCalledWith("abc123"));
    stop();
  });

  it("starts a new chat and sends a prompt from the surface", async () => {
    const stop = startCliOpenBridge();
    emit("projectOpen", {
      path: "/work/demo",
      surface: `chat:${JSON.stringify({ local: null, prompt: "draw an ox with local/iris-3b-turbo" })}`,
    });
    await vi.waitFor(() =>
      expect(runTurn).toHaveBeenCalledWith(expect.anything(), "draw an ox with local/iris-3b-turbo", []),
    );
    stop();
  });

  it("starts the new chat on a local model when the launch names one", async () => {
    const stop = startCliOpenBridge();
    emit("projectOpen", {
      path: "/work/demo",
      surface: `chat:${JSON.stringify({ local: "gemma-4-26b-a4b-q3", prompt: "hi" })}`,
    });
    await vi.waitFor(() => expect(useLocalModel).toHaveBeenCalledWith("gemma-4-26b-a4b-q3"));
    await vi.waitFor(() => expect(runTurn).toHaveBeenCalledWith(expect.anything(), "hi", []));
    stop();
  });

  it("says why in the chat when the named local model can't start", async () => {
    useLocalModel.mockRejectedValueOnce("gemma-x isn't downloaded yet");
    const stop = startCliOpenBridge();
    emit("projectOpen", { path: "/work/demo", surface: `chat:${JSON.stringify({ local: "gemma-x", prompt: "hi" })}` });
    await vi.waitFor(() => {
      const s = useStore.getState();
      const thread = s.session ? s.threads[s.session.session_id] ?? [] : [];
      expect(JSON.stringify(thread)).toContain("Couldn't start gemma-x: gemma-x isn't downloaded yet");
    });
    expect(runTurn).not.toHaveBeenCalled();
    stop();
  });

  it("applies a cold start's surface once the bridge is up", async () => {
    takeLaunchSurface.mockResolvedValueOnce("settings:tools");
    const stop = startCliOpenBridge();
    await vi.waitFor(() => expect(useStore.getState().settingsPage).toBe("tools"));
    stop();
  });
});
