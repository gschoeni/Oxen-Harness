import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ProjectsPage } from "./ProjectsPage";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { Project, SessionSummary } from "../../lib/types";

const sessions: SessionSummary[] = [
  { id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "First chat", message_count: 4, review_status: "" },
  { id: "e1", workspace: "/elsewhere", model: "m", created_at: 1_700_000_000, title: "Elsewhere chat", message_count: 1, review_status: "" },
];
const projects: Project[] = [
  { path: "/w", name: "w", session_count: 1, active: true },
  { path: "/elsewhere", name: "elsewhere", session_count: 1, active: false },
];

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
    sessions,
    projects,
    projectsOpen: true,
  });
});

describe("ProjectsPage", () => {
  it("lists every project, marking the current one", () => {
    render(<ProjectsPage />);
    expect(screen.getByText("w")).toBeInTheDocument();
    expect(screen.getByText("elsewhere")).toBeInTheDocument();
    expect(screen.getByText("current")).toBeInTheDocument();
    expect(screen.getByText("w").closest(".project-card")).toHaveClass("active");
  });

  it("enters a project and starts a fresh chat there", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("elsewhere"));
    expect(ipc.setActiveProject).toHaveBeenCalledWith("/elsewhere");
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(useStore.getState().projectsOpen).toBe(false);
  });

  it("opens the folder picker from the new-project card", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Open a folder…"));
    expect(ipc.pickFolder).toHaveBeenCalledOnce();
  });

  it("closes back to the chat with the close button and Escape", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByRole("button", { name: /back to chat/i }));
    expect(useStore.getState().projectsOpen).toBe(false);

    useStore.setState({ projectsOpen: true });
    await userEvent.keyboard("{Escape}");
    expect(useStore.getState().projectsOpen).toBe(false);
  });

  it("cannot be dismissed when no project is open", () => {
    useStore.setState({ session: null });
    render(<ProjectsPage />);
    expect(screen.queryByRole("button", { name: /back to chat/i })).toBeNull();
  });

  it("shows a running indicator on projects with mid-run chats", () => {
    useStore.setState({ runStatus: { e1: "running" } });
    render(<ProjectsPage />);
    const card = screen.getByText("elsewhere").closest(".project-card")!;
    expect(card.querySelector(".project-card-running")).not.toBeNull();
    const other = screen.getByText("w").closest(".project-card")!;
    expect(other.querySelector(".project-card-running")).toBeNull();
  });
});
