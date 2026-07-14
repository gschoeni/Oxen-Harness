import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Sidebar } from "./Sidebar";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { Project, SessionSummary } from "../../lib/types";

const sessions: SessionSummary[] = [
  { id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "First chat", message_count: 4, review_status: "" },
  { id: "s2", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Second chat", message_count: 2, review_status: "" },
  { id: "other", workspace: "/elsewhere", model: "m", created_at: 1_700_000_000, title: "Other project chat", message_count: 1, review_status: "" },
];
const projects: Project[] = [
  { path: "/w", name: "w", description: "", instructions: "", context: [], session_count: 2, active: true },
  { path: "/elsewhere", name: "elsewhere", description: "", instructions: "", context: [], session_count: 1, active: false },
];

beforeEach(() => {
  resetAll();
  // The active chat lives in project "/w", so the sidebar is scoped to it.
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
    sessions,
    projects,
  });
});

describe("Sidebar", () => {
  it("shows the active project's name and only its chats, marking the active one", () => {
    render(<Sidebar />);
    expect(screen.getByText("w", { selector: ".current-project-name" })).toBeInTheDocument();
    expect(screen.getByText("First chat")).toBeInTheDocument();
    expect(screen.getByText("Second chat")).toBeInTheDocument();
    expect(screen.queryByText("Other project chat")).toBeNull();
    expect(screen.getByText("First chat").closest(".history-item")).toHaveClass("active");
  });

  it("pins a brand-new (untitled) active chat to the top of the list", () => {
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "fresh", workspace: "/w" } });
    render(<Sidebar />);
    const newChat = screen.getByText("New chat", { selector: ".history-title" });
    expect(newChat.closest(".history-item")).toHaveClass("active");
  });

  it("starts a new session when New chat is clicked", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(ipc.newSession).toHaveBeenCalledOnce();
  });

  it("resumes a chat when its row is clicked", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByText("Second chat"));
    expect(ipc.resumeSession).toHaveBeenCalledWith("s2");
  });

  it("opens the projects page from the Projects link", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: /all projects/i }));
    expect(useStore.getState().projectsOpen).toBe(true);
  });

  it("signals activity in other projects on the Projects link", () => {
    useStore.setState({ runStatus: { other: "running" } });
    render(<Sidebar />);
    expect(document.querySelector(".projects-link-dot")).not.toBeNull();
  });

  it("shows no activity dot when only the current project is busy", () => {
    useStore.setState({ runStatus: { s2: "running" } });
    render(<Sidebar />);
    expect(document.querySelector(".projects-link-dot")).toBeNull();
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

  it("shows the model and date for each chat", () => {
    useStore.setState({
      sessions: [
        { id: "s1", workspace: "/w", model: "anthropic/claude-sonnet-4-5-20250929", created_at: 1_700_000_000, title: "First chat", message_count: 4, review_status: "" },
      ],
    });
    render(<Sidebar />);
    const row = screen.getByText("First chat").closest(".history-item")!;
    // Provider prefix and date suffix are trimmed for a compact label.
    expect(row.querySelector(".history-model")).toHaveTextContent("claude-sonnet-4-5");
    expect(row.querySelector(".history-date")).not.toBeNull();
  });

  it("deletes a chat only after confirming in the modal", async () => {
    render(<Sidebar />);
    // The delete icon opens a confirmation modal rather than deleting outright.
    await userEvent.click(screen.getByRole("button", { name: "Delete chat: Second chat" }));
    expect(ipc.deleteSession).not.toHaveBeenCalled();
    expect(screen.getByText("Delete chat?")).toBeInTheDocument();

    // Cancelling closes the modal and deletes nothing.
    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByText("Delete chat?")).toBeNull();
    expect(ipc.deleteSession).not.toHaveBeenCalled();

    // Confirming deletes the right session.
    await userEvent.click(screen.getByRole("button", { name: "Delete chat: Second chat" }));
    await userEvent.click(screen.getByRole("button", { name: /^delete$/i }));
    expect(ipc.deleteSession).toHaveBeenCalledWith("s2");
  });
});
