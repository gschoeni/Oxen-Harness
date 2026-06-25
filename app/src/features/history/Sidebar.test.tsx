import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Sidebar } from "./Sidebar";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { SessionSummary } from "../../lib/types";

const sessions: SessionSummary[] = [
  { id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "First chat", message_count: 4 },
  { id: "s2", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Second chat", message_count: 2 },
];

beforeEach(() => {
  resetAll();
  useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1" }, sessions });
});

describe("Sidebar", () => {
  it("lists chat history and marks the active session", () => {
    render(<Sidebar />);
    expect(screen.getByText("First chat")).toBeInTheDocument();
    expect(screen.getByText("Second chat")).toBeInTheDocument();
    const active = screen.getByText("First chat").closest(".history-item");
    expect(active).toHaveClass("active");
  });

  it("pins a brand-new (untitled) active session to the top", () => {
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "fresh" }, sessions });
    render(<Sidebar />);
    const newChat = screen.getByText("New chat", { selector: ".history-title" });
    expect(newChat).toBeInTheDocument();
    expect(newChat.closest(".history-item")).toHaveClass("active");
  });

  it("starts a new session when New chat is clicked", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: /new chat/i }));
    expect(ipc.newSession).toHaveBeenCalledOnce();
  });

  it("resumes a session when a history row is clicked", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByText("Second chat"));
    expect(ipc.resumeSession).toHaveBeenCalledWith("s2");
  });

  it("opens Settings from the footer button", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: /settings/i }));
    expect(useStore.getState().settingsOpen).toBe(true);
  });

  it("shows a running indicator and an unread dot per chat", () => {
    useStore.setState({ runStatus: { s1: "running", s2: "unread" } });
    render(<Sidebar />);
    const first = screen.getByText("First chat").closest(".history-item")!;
    const second = screen.getByText("Second chat").closest(".history-item")!;
    expect(first.querySelector(".chat-status.running")).not.toBeNull();
    expect(second.querySelector(".chat-status.unread")).not.toBeNull();
  });

  it("lets you start a new chat even while one is running in the background", async () => {
    useStore.setState({ runStatus: { s1: "running" } });
    render(<Sidebar />);
    const newChat = screen.getByRole("button", { name: /new chat/i });
    expect(newChat).not.toBeDisabled();
    await userEvent.click(newChat);
    expect(ipc.newSession).toHaveBeenCalledOnce();
  });
});
