import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
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
  {
    path: "/w",
    name: "Writer",
    description: "A calm place to draft essays.",
    instructions: "Use plain language.",
    context: [{ path: ".oxen-harness/context/brief.md", name: "brief.md", kind: "text", size_bytes: 42 }],
    session_count: 1,
    active: true,
  },
  { path: "/elsewhere", name: "Elsewhere", description: "", instructions: "", context: [], session_count: 1, active: false },
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
  it("lists every project and marks the current one", () => {
    render(<ProjectsPage />);
    expect(screen.getByText("Writer")).toBeInTheDocument();
    expect(screen.getByText("Elsewhere")).toBeInTheDocument();
    expect(screen.getByText("current")).toBeInTheDocument();
    expect(screen.getByText("Writer").closest(".project-card")).toHaveClass("active");
  });

  it("opens a project home with durable instructions and context", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));

    expect(await screen.findByRole("heading", { name: "Writer" })).toBeInTheDocument();
    expect(screen.getByText("A calm place to draft essays.")).toBeInTheDocument();
    expect(screen.getByText("Use plain language.")).toBeInTheDocument();
    expect(screen.getByText("brief.md")).toBeInTheDocument();
    expect(ipc.setActiveProject).toHaveBeenCalledWith("/w");
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(useStore.getState().projectsOpen).toBe(true);
  });

  it("creates an existing-folder project with guidance and context", async () => {
    ipc.pickFolder.mockResolvedValueOnce("/work/demo");
    ipc.pickProjectContext.mockResolvedValueOnce(["/tmp/brief.md"]);
    render(<ProjectsPage />);

    await userEvent.click(screen.getByRole("button", { name: /start a project/i }));
    await userEvent.click(screen.getByRole("button", { name: /use existing folder/i }));
    await userEvent.click(screen.getByRole("button", { name: /choose project folder/i }));
    await userEvent.type(screen.getByLabelText("Project name"), "Demo App");
    await userEvent.type(screen.getByLabelText("Project goal"), "Ship a focused single-page app.");
    await userEvent.type(screen.getByLabelText("Project instructions"), "Keep the interface accessible.");
    await userEvent.click(screen.getByRole("button", { name: /add context/i }));
    await userEvent.click(screen.getByRole("button", { name: "Create project" }));

    expect(ipc.startProject).toHaveBeenCalledWith({
      name: "Demo App",
      description: "Ship a focused single-page app.",
      instructions: "Keep the interface accessible.",
      directory: "/work/demo",
      createDirectory: false,
      contextPaths: ["/tmp/brief.md"],
    });
    expect(await screen.findByRole("heading", { name: "Demo App" })).toBeInTheDocument();
  });

  it("edits project guidance and starts a fresh prompt context", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });
    ipc.newSession.mockClear();

    await userEvent.click(screen.getByRole("button", { name: "Edit project" }));
    const instructions = screen.getByLabelText("Project instructions");
    await userEvent.clear(instructions);
    await userEvent.type(instructions, "Write for curious beginners.");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));

    expect(ipc.updateProject).toHaveBeenCalledWith(
      "/w",
      "Writer",
      "A calm place to draft essays.",
      "Write for curious beginners.",
    );
    expect(ipc.newSession).toHaveBeenCalledOnce();
  });

  it("adds and removes durable project context", async () => {
    ipc.pickProjectContext.mockResolvedValueOnce(["/tmp/research.pdf"]);
    ipc.addProjectContext.mockResolvedValueOnce({
      ...projects[0],
      context: [...projects[0].context, { path: ".oxen-harness/context/research.pdf", name: "research.pdf", kind: "pdf", size_bytes: 1200 }],
    });
    ipc.removeProjectContext.mockResolvedValueOnce({ ...projects[0], context: [] });
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });

    await userEvent.click(screen.getByRole("button", { name: /add project context/i }));
    expect(await screen.findByText("research.pdf")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /remove research\.pdf/i }));

    expect(ipc.addProjectContext).toHaveBeenCalledWith("/w", ["/tmp/research.pdf"]);
    expect(ipc.removeProjectContext).toHaveBeenCalledWith("/w", ".oxen-harness/context/research.pdf");
    await waitFor(() => expect(screen.queryByText("research.pdf")).toBeNull());
  });

  it("sends the project-home prompt into the prepared chat", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });
    await userEvent.type(screen.getByLabelText("Ask about this project"), "Build the landing page");
    await userEvent.click(screen.getByRole("button", { name: "Send project prompt" }));

    expect(useStore.getState().projectsOpen).toBe(false);
    const session = useStore.getState().session!;
    expect(useStore.getState().threads[session.session_id]?.[0]).toMatchObject({
      kind: "user",
      text: "Build the landing page",
    });
  });

  it("is a non-dismissible navigation root", async () => {
    render(<ProjectsPage />);
    await userEvent.keyboard("{Escape}");
    expect(useStore.getState().projectsOpen).toBe(true);
    expect(screen.queryByRole("button", { name: /back to chat/i })).toBeNull();
  });

  it("shows a running indicator on projects with mid-run chats", () => {
    useStore.setState({ runStatus: { e1: "running" } });
    render(<ProjectsPage />);
    expect(screen.getByText("Elsewhere").closest(".project-card")?.querySelector(".project-card-running")).not.toBeNull();
    expect(screen.getByText("Writer").closest(".project-card")?.querySelector(".project-card-running")).toBeNull();
  });
});
