import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { TasksPanel } from "./TasksPanel";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
  });
});

describe("TasksPanel", () => {
  it("lists the chat's background commands from list events and stops one", async () => {
    const { container } = render(<TasksPanel />);
    expect(container).toBeEmptyDOMElement();
    act(() =>
      useStore.getState().ingestTasksChanged({
        session: "s1",
        tasks: [
          {
            id: 3,
            command: "npm run dev",
            running: true,
            exit_code: null,
            killed: false,
            elapsed_secs: 42,
            last_line: "ready on :5173",
          },
          {
            id: 2,
            command: "cargo test",
            running: false,
            exit_code: 0,
            killed: false,
            elapsed_secs: 9,
            last_line: "test result: ok",
          },
        ],
      }),
    );
    expect(screen.getByText("1 background command running")).toBeInTheDocument();
    expect(screen.getByText("ready on :5173")).toBeInTheDocument();
    expect(screen.getByText("42s")).toBeInTheDocument();
    // Only the running one offers a stop.
    expect(screen.queryByRole("button", { name: "Stop cargo test" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Stop npm run dev" }));
    expect(ipc.killBackgroundTask).toHaveBeenCalledWith("s1", 3);
    // An empty list closes the panel.
    act(() => useStore.getState().ingestTasksChanged({ session: "s1", tasks: [] }));
    expect(container).toBeEmptyDOMElement();
  });
});
