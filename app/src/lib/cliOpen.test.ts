import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { emit, setActiveProject } from "../test/ipcMock";
import { resetAll } from "../test/utils";
import { startCliOpenBridge } from "./cliOpen";
import { useStore } from "./store";

beforeEach(resetAll);

describe("cli open bridge", () => {
  it("enters the forwarded project and leaves the home takeover", async () => {
    const stop = startCliOpenBridge();
    useStore.getState().setHomeOpen(true);

    emit("projectOpen", "/work/demo");

    await vi.waitFor(() => expect(setActiveProject).toHaveBeenCalledWith("/work/demo"));
    await vi.waitFor(() => expect(useStore.getState().homeOpen).toBe(false));
    stop();
  });
});
