import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { emit, resumeSession, setActiveProject, takeLaunchSurface } from "../test/ipcMock";
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

  it("applies a cold start's surface once the bridge is up", async () => {
    takeLaunchSurface.mockResolvedValueOnce("settings:tools");
    const stop = startCliOpenBridge();
    await vi.waitFor(() => expect(useStore.getState().settingsPage).toBe("tools"));
    stop();
  });
});
