import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("./lib/ipc", () => import("./test/ipcMock"));

import { TitleBar } from "./TitleBar";
import { useStore } from "./lib/store";
import { resetAll } from "./test/utils";

beforeEach(() => {
  resetAll();
});

describe("TitleBar running work indicator", () => {
  it("counts active fleet lanes instead of double-counting their parent sessions", () => {
    useStore.setState({
      threadsSnapshot: { entries: [], running: ["persisted"] },
      runStatus: { parent: "running", solo: "running", finished: "unread" },
      fleets: {
        "fleet-1": {
          session: "parent",
          source: "turn",
          focused: null,
          lanes: [
            { name: "one", id: "l1", status: "running", activity: "", tail: "", tokens: 0 },
            { name: "two", id: "", status: "queued", activity: "", tail: "", tokens: 0 },
            { name: "three", id: "l3", status: "done", activity: "", tail: "", tokens: 10 },
          ],
        },
        // A second fleet in the same chat adds its lanes, not a second session.
        "fleet-2": {
          session: "parent",
          source: "turn",
          focused: null,
          lanes: [{ name: "four", id: "l4", status: "running", activity: "", tail: "", tokens: 0 }],
        },
      },
    });

    render(<TitleBar />);

    // Three active fleet lanes + one solo live session + one backend-known session.
    expect(screen.getByRole("button", { name: "5 running — go Home" })).toBeTruthy();
  });

  it("lets locally completed work override an older snapshot", () => {
    useStore.setState({
      threadsSnapshot: { entries: [], running: ["done", "still-running"] },
      runStatus: { done: "unread", "still-running": "running" },
    });

    render(<TitleBar />);

    expect(screen.getByRole("button", { name: "1 running — go Home" })).toBeTruthy();
  });

  it("goes Home when clicked, closing Settings and any project page", async () => {
    useStore.setState({ homeOpen: false, projectHomePath: "/work/project", settingsOpen: true });
    render(<TitleBar />);

    await userEvent.click(screen.getByRole("button", { name: "0 running — go Home" }));

    expect(useStore.getState().homeOpen).toBe(true);
    expect(useStore.getState().settingsOpen).toBe(false);
    expect(useStore.getState().projectHomePath).toBeNull();
  });
});
