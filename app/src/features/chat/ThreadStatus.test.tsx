import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ThreadStatus } from "./ThreadStatus";
import { useStore } from "../../lib/store";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";
import type { ThreadEntry } from "../../lib/types";

const NOW = Math.floor(Date.now() / 1000);

function entry(overrides: Partial<ThreadEntry> = {}): ThreadEntry {
  return {
    id: "cur",
    workspace: "/work/app",
    model: "m",
    created_at: NOW - 7_200,
    last_activity_at: NOW - 600,
    title: "fix the flaky test",
    last_reply: "",
    message_count: 8,
    mid_turn: false,
    finished_at: 0,
    review_status: "",
    seen_at: 0,
    ...overrides,
  };
}

function seedChat(e: ThreadEntry, running: string[] = []) {
  const snapshot = { entries: [e], running };
  useStore.setState({
    threadsSnapshot: snapshot,
    session: {
      model: "m",
      workspace: "/work/app",
      session_id: e.id,
      tokens_used: 0,
      context_tokens: 0,
      context_window: 200_000,
      compression_mode: "off",
      permission_mode: "relaxed",
    },
  });
  vi.mocked(ipc.threadsSnapshot).mockResolvedValue(snapshot);
}

beforeEach(() => {
  resetAll();
});

describe("the chat's status strip", () => {
  it("shows where the thread stands and marks it finished in one click", async () => {
    seedChat(entry());
    render(<ThreadStatus />);
    expect(screen.getByText(/last reply/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Mark finished" }));
    await waitFor(() => expect(ipc.sessionFinish).toHaveBeenCalledWith("cur"));
  });

  it("offers no finish while the agent is running", () => {
    seedChat(entry(), ["cur"]);
    render(<ThreadStatus />);
    expect(screen.getByText("running")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Mark finished" })).toBeNull();
  });

  it("says when the agent is parked on an approval", () => {
    seedChat(entry(), ["cur"]);
    useStore.setState({ approvals: { cur: { id: "a1", session: "cur" } as never } });
    render(<ThreadStatus />);
    expect(screen.getByText("waiting on your approval")).toBeTruthy();
  });

  it("a finished thread can be reopened from its chat", async () => {
    seedChat(entry({ finished_at: NOW - 60 }));
    render(<ThreadStatus />);
    expect(screen.getByText(/^finished ·/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Reopen" }));
    await waitFor(() => expect(ipc.sessionReopen).toHaveBeenCalledWith("cur"));
  });

  it("leads with a session id copy chip", async () => {
    seedChat(entry({ id: "sess-0123456789abcdef" }));
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const { container } = render(<ThreadStatus />);
    const chip = screen.getByRole("button", { name: /copy session id sess-0123456789abcdef/i });
    expect(container.querySelector(".chat-status")?.firstElementChild).toBe(chip);
    expect(chip.textContent).toContain("…89abcdef");
    await userEvent.click(chip);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("sess-0123456789abcdef"));
    expect(await screen.findByText("copied")).toBeTruthy();
    expect(ipc.sessionFinish).not.toHaveBeenCalled();
  });

  it("renders nothing for a session the snapshot doesn't know", () => {
    useStore.setState({ threadsSnapshot: { entries: [], running: [] } });
    const { container } = render(<ThreadStatus />);
    expect(container.querySelector(".chat-status")).toBeNull();
  });
});
