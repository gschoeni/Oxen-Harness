import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { WorkEmpty, WORK_STARTERS } from "./WorkEmpty";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaItem } from "../../lib/types";

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
    dockCollapsed: { left: true },
  });
});

describe("WorkEmpty", () => {
  it("sends a chip's text to the agent as written", async () => {
    const send = vi.fn();
    useStore.setState({ send });
    render(<WorkEmpty workspace="/w" />);
    const chips = screen.getAllByRole("button", { name: /./ }).map((b) => b.textContent);
    for (const prompt of WORK_STARTERS) expect(chips).toContain(prompt);
    await userEvent.click(screen.getByRole("button", { name: WORK_STARTERS[1] }));
    expect(send).toHaveBeenCalledWith(WORK_STARTERS[1]);
  });

  it("offers the file tree, and the gallery only once the project has generations", async () => {
    render(<WorkEmpty workspace="/w" />);
    expect(ipc.listMedia).toHaveBeenCalledWith("/w");
    expect(screen.queryByRole("button", { name: /Gallery/ })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Open a file…" }));
    expect(useStore.getState().leftTab).toBe("files");
    expect(useStore.getState().dockCollapsed.left).toBe(false);

    act(() =>
      useStore.setState({
        media: { "/w": [{ id: "g1", session: "s1", kind: "image", status: "succeeded" } as MediaItem] },
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: /Gallery · 1/ }));
    expect(useStore.getState().workContexts.s1.current).toEqual({ view: "gallery" });
  });
});
