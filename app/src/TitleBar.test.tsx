import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("./lib/ipc", () => import("./test/ipcMock"));

import { TitleBar } from "./TitleBar";
import { useStore } from "./lib/store";
import { getUi, setUi } from "./lib/uiState";
import { resetAll } from "./test/utils";

beforeEach(() => {
  resetAll();
});

describe("TitleBar running work indicator", () => {
  it("counts active fleet lanes instead of double-counting their parent sessions", () => {
    useStore.setState({
      ledger: { entries: [], running: ["persisted"], last_seen: 0 },
      runStatus: { parent: "running", solo: "running", finished: "unread" },
      fleets: {
        parent: {
          source: "turn",
          focused: null,
          lanes: [
            { name: "one", status: "running", activity: "", tail: "", tokens: 0 },
            { name: "two", status: "queued", activity: "", tail: "", tokens: 0 },
            { name: "three", status: "done", activity: "", tail: "", tokens: 10 },
          ],
        },
      },
    });

    render(<TitleBar />);

    // Two active fleet lanes + one solo live session + one backend-known session.
    expect(screen.getByRole("button", { name: "4 running — open the Ledger" })).toBeTruthy();
  });

  it("lets locally completed work override an older Ledger snapshot", () => {
    useStore.setState({
      ledger: { entries: [], running: ["done", "still-running"], last_seen: 0 },
      runStatus: { done: "unread", "still-running": "running" },
    });

    render(<TitleBar />);

    expect(screen.getByRole("button", { name: "1 running — open the Ledger" })).toBeTruthy();
  });

  it("opens the Ledger lens when clicked", async () => {
    setUi("homeView", "cards");
    useStore.setState({ homeOpen: false, projectHomePath: "/work/project", settingsOpen: true });
    render(<TitleBar />);

    await userEvent.click(screen.getByRole("button", { name: "0 running — open the Ledger" }));

    expect(useStore.getState().homeOpen).toBe(true);
    expect(useStore.getState().settingsOpen).toBe(false);
    expect(useStore.getState().projectHomePath).toBeNull();
    expect(getUi("homeView")).toBe("ledger");
  });
});
