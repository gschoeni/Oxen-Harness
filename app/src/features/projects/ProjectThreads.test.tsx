import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ProjectThreads } from "./ProjectThreads";
import { useStore } from "../../lib/store";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";
import type { SessionSummary, ThreadEntry } from "../../lib/types";

const NOW = Math.floor(Date.now() / 1000);

function entry(id: string, overrides: Partial<ThreadEntry> = {}): ThreadEntry {
  return {
    id,
    workspace: "/w",
    model: "m",
    created_at: NOW - 7_200,
    last_activity_at: NOW - 600,
    title: id,
    last_reply: "",
    message_count: 4,
    mid_turn: false,
    finished_at: 0,
    review_status: "",
    seen_at: 0,
    ...overrides,
  };
}

function summary(id: string, workspace = "/w"): SessionSummary {
  return { id, workspace, model: "m", created_at: NOW - 7_200, title: id, message_count: 4, review_status: "", source: "" };
}

beforeEach(() => {
  resetAll();
  const snapshot = {
    entries: [
      entry("dangling", { mid_turn: true }),
      entry("calm"),
      entry("busy", { mid_turn: true }),
      entry("done", { finished_at: NOW - 60 }),
    ],
    running: ["busy"],
  };
  useStore.setState({
    threadsSnapshot: snapshot,
    sessions: [summary("dangling"), summary("calm"), summary("busy"), summary("done"), summary("elsewhere", "/other")],
  });
  vi.mocked(ipc.threadsSnapshot).mockResolvedValue(snapshot);
});

describe("a project's chats", () => {
  it("bands this project's chats: needs you, then the rest, finished last", () => {
    render(<ProjectThreads workspace="/w" />);
    const heads = [...document.querySelectorAll(".project-threads-head")].map((h) => h.textContent);
    expect(heads).toEqual(["Needs you 1", "Other chats", "Finished 1"]);
    expect(screen.getByText("left dangling — reply never arrived")).toBeTruthy();
    expect(screen.queryByText("elsewhere")).toBeNull();
    const titles = [...document.querySelectorAll(".project-thread-title")].map((t) => t.textContent);
    expect(titles).toEqual(["dangling", "calm", "busy", "done"]);
  });

  it("opens a chat and leaves Home", async () => {
    render(<ProjectThreads workspace="/w" />);
    await userEvent.click(screen.getByRole("button", { name: /^calm/ }));
    await waitFor(() => expect(ipc.resumeSession).toHaveBeenCalledWith("calm"));
    await waitFor(() => expect(useStore.getState().homeOpen).toBe(false));
  });

  it("marks an idle chat finished, never a running one, and reopens a finished one", async () => {
    render(<ProjectThreads workspace="/w" />);
    const rowOf = (title: string) => screen.getByText(title).closest(".project-thread")!;
    expect(rowOf("busy").querySelector(".project-thread-action")).toBeNull();

    await userEvent.click(rowOf("calm").querySelector(".project-thread-action") as HTMLElement);
    await waitFor(() => expect(ipc.sessionFinish).toHaveBeenCalledWith("calm"));

    expect(rowOf("done").querySelector(".project-thread-action")?.textContent).toContain("Reopen");
    await userEvent.click(rowOf("done").querySelector(".project-thread-action") as HTMLElement);
    await waitFor(() => expect(ipc.sessionReopen).toHaveBeenCalledWith("done"));
  });

  it("renders nothing for a project with no chats", () => {
    const { container } = render(<ProjectThreads workspace="/nowhere" />);
    expect(container.querySelector(".project-threads")).toBeNull();
  });
});
