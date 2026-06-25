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
  { id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "First chat", message_count: 4 },
  { id: "s2", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Second chat", message_count: 2 },
];
const projects: Project[] = [{ path: "/w", name: "w", session_count: 2, active: true }];

beforeEach(() => {
  resetAll();
  // The active chat lives in project "/w", so that folder is expanded by default.
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
    sessions,
    projects,
  });
});

describe("Sidebar", () => {
  it("lists chats under the active project folder and marks the active chat", () => {
    render(<Sidebar />);
    expect(screen.getByText("w", { selector: ".project-name" })).toBeInTheDocument();
    expect(screen.getByText("First chat")).toBeInTheDocument();
    expect(screen.getByText("Second chat")).toBeInTheDocument();
    expect(screen.getByText("First chat").closest(".history-item")).toHaveClass("active");
  });

  it("pins a brand-new (untitled) active chat into its project", () => {
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "fresh", workspace: "/w" } });
    render(<Sidebar />);
    const newChat = screen.getByText("New chat", { selector: ".history-title" });
    expect(newChat.closest(".history-item")).toHaveClass("active");
  });

  it("starts a new session when the top New chat is clicked", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(ipc.newSession).toHaveBeenCalledOnce();
  });

  it("resumes a chat when its row is clicked", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByText("Second chat"));
    expect(ipc.resumeSession).toHaveBeenCalledWith("s2");
  });

  it("collapses a project folder to hide its chats", async () => {
    render(<Sidebar />);
    expect(screen.getByText("First chat")).toBeInTheDocument();
    await userEvent.click(screen.getByText("w", { selector: ".project-name" }));
    expect(screen.queryByText("First chat")).toBeNull();
  });

  it("opens the projects screen from the header", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: /manage projects/i }));
    expect(useStore.getState().projectsOpen).toBe(true);
  });

  it("starts a new chat in a project from its folder", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: /new chat in w/i }));
    expect(ipc.setActiveProject).toHaveBeenCalledWith("/w");
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
});
