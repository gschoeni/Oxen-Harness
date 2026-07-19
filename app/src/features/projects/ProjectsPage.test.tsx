import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ProjectsPage } from "./ProjectsPage";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { Project, SessionSummary } from "../../lib/types";

const writerSession: SessionSummary = {
  id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "First chat", message_count: 4, review_status: "", source: "",
};
const sessions: SessionSummary[] = [
  { id: "e1", workspace: "/elsewhere", model: "m", created_at: 1_700_000_000, title: "Elsewhere chat", message_count: 1, review_status: "", source: "" },
];
const projects: Project[] = [
  {
    path: "/w",
    name: "Writer",
    description: "A calm place to draft essays.",
    instructions: "Use plain language.",
    context: [{ path: ".oxen-harness/context/brief.md", name: "brief.md", kind: "text", size_bytes: 42 }],
    session_count: 0,
    active: true,
    last_used_at: null,
  },
  { path: "/elsewhere", name: "Elsewhere", description: "", instructions: "", context: [], session_count: 1, active: false, last_used_at: null },
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

  it("resumes the newest chat when an established project is selected", async () => {
    const latest = { ...writerSession, id: "latest", created_at: 1_800_000_000, title: "Latest chat" };
    useStore.setState({
      projects: [{ ...projects[0], session_count: 2 }, projects[1]],
      sessions: [latest, writerSession, ...sessions],
    });
    ipc.resumeSession.mockResolvedValueOnce({
      info: { ...ipc.sampleSession, session_id: "latest", workspace: "/w" },
      messages: [],
      running: false,
    });
    render(<ProjectsPage />);

    await userEvent.click(screen.getByText("Writer"));

    expect(ipc.resumeSession).toHaveBeenCalledWith("latest");
    expect(useStore.getState().projectsOpen).toBe(false);
    expect(screen.queryByRole("heading", { name: "Writer" })).toBeNull();
  });

  it("opens getting started for a project without chat history", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));

    expect(await screen.findByRole("heading", { name: "Writer" })).toBeInTheDocument();
    expect(screen.getByText("A calm place to draft essays.")).toBeInTheDocument();
    expect(screen.getByText("Use plain language.")).toBeInTheDocument();
    expect(screen.getByText("brief.md")).toBeInTheDocument();
    expect(ipc.setActiveProject).toHaveBeenCalledWith("/w");
    expect(ipc.newSession).not.toHaveBeenCalled();
    expect(useStore.getState().projectsOpen).toBe(true);
  });

  it("opens an established project's files and settings when explicitly targeted", () => {
    useStore.setState({
      projects: [{ ...projects[0], session_count: 1 }, projects[1]],
      sessions: [writerSession, ...sessions],
      projectHomePath: "/w",
    });

    render(<ProjectsPage />);

    expect(screen.getByRole("heading", { name: "Writer" })).toBeInTheDocument();
    expect(screen.getByText("Use plain language.")).toBeInTheDocument();
    expect(screen.getByText("brief.md")).toBeInTheDocument();
    expect(ipc.resumeSession).not.toHaveBeenCalled();
  });

  it("creates an existing-folder project and drops straight into a fresh chat", async () => {
    ipc.pickFolder.mockResolvedValueOnce("/work/demo");
    render(<ProjectsPage />);

    await userEvent.click(screen.getByRole("button", { name: /start a project/i }));
    expect(screen.queryByLabelText("Project instructions")).toBeNull();
    expect(screen.queryByText(/starting context/i)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /use existing folder/i }));
    await userEvent.click(screen.getByRole("button", { name: /choose project folder/i }));
    await userEvent.type(screen.getByLabelText("Project name"), "Demo App");
    await userEvent.type(screen.getByLabelText("Project goal"), "Ship a focused single-page app.");
    await userEvent.click(screen.getByRole("button", { name: "Create project" }));

    expect(ipc.startProject).toHaveBeenCalledWith({
      name: "Demo App",
      description: "Ship a focused single-page app.",
      directory: "/work/demo",
      createDirectory: false,
    });
    // No settings/home detour: the project is activated, a fresh chat starts,
    // and the projects overlay closes.
    await waitFor(() => expect(useStore.getState().projectsOpen).toBe(false));
    expect(ipc.setActiveProject).toHaveBeenCalledWith("/work/demo");
    expect(ipc.newSession).toHaveBeenCalledOnce();
    expect(screen.queryByRole("heading", { name: "Demo App" })).toBeNull();
  });

  it("prefills the saved default parent when creating a project", async () => {
    ipc.getDefaultProjectLocation.mockResolvedValueOnce("/work/Projects");
    render(<ProjectsPage />);

    await userEvent.click(screen.getByRole("button", { name: /start a project/i }));
    expect(await screen.findByText("/work/Projects")).toBeInTheDocument();
    expect(screen.getByText("Default project location")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Project name"), "Demo App");
    await userEvent.click(screen.getByRole("button", { name: "Create project" }));

    expect(ipc.startProject).toHaveBeenCalledWith({
      name: "Demo App",
      description: "",
      directory: "/work/Projects",
      createDirectory: true,
    });
  });

  it("lets a chosen parent folder become the project default", async () => {
    ipc.pickProjectParent.mockResolvedValueOnce("/work/Projects");
    render(<ProjectsPage />);

    await userEvent.click(screen.getByRole("button", { name: /start a project/i }));
    await userEvent.click(screen.getByRole("button", { name: /choose project folder/i }));
    await userEvent.click(screen.getByRole("button", { name: "Use as default" }));

    expect(ipc.setDefaultProjectLocation).toHaveBeenCalledWith("/work/Projects");
    expect(await screen.findByText("Default project location")).toBeInTheDocument();
  });

  it("does not race project creation or replace an existing-folder choice while saving a default", async () => {
    let finishSaving!: (path: string) => void;
    ipc.pickProjectParent.mockResolvedValueOnce("/work/Projects");
    ipc.setDefaultProjectLocation.mockImplementationOnce(() => new Promise((resolve) => { finishSaving = resolve; }));
    render(<ProjectsPage />);

    await userEvent.click(screen.getByRole("button", { name: /start a project/i }));
    await userEvent.type(screen.getByLabelText("Project name"), "Demo App");
    await userEvent.click(screen.getByRole("button", { name: /choose project folder/i }));
    await userEvent.click(screen.getByRole("button", { name: "Use as default" }));
    expect(screen.getByRole("button", { name: "Create project" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: /use existing folder/i }));
    await act(async () => { finishSaving("/work/Projects"); });

    expect(screen.getByText("Choose the project folder…")).toBeInTheDocument();
  });

  it("edits the project name and goal directly on the page", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });
    ipc.newSession.mockClear();

    expect(screen.queryByRole("button", { name: "Edit project" })).toBeNull();
    const name = screen.getByLabelText("Project name");
    const goal = screen.getByLabelText("Project goal");
    await userEvent.clear(name);
    await userEvent.type(name, "Writer Studio");
    await userEvent.clear(goal);
    await userEvent.type(goal, "Draft thoughtful essays for curious readers.");
    await userEvent.click(screen.getByRole("button", { name: "Save project details" }));

    expect(ipc.updateProject).toHaveBeenCalledWith(
      "/w",
      "Writer Studio",
      "Draft thoughtful essays for curious readers.",
      "Use plain language.",
    );
    expect(ipc.newSession).not.toHaveBeenCalled();
  });

  it("edits project instructions from their focused control", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });
    ipc.newSession.mockClear();

    await userEvent.click(screen.getByRole("button", { name: "Edit project instructions" }));
    const instructions = screen.getByLabelText("Project instructions");
    await userEvent.clear(instructions);
    await userEvent.type(instructions, "Write for curious beginners.");
    await userEvent.click(screen.getByRole("button", { name: "Save instructions" }));

    expect(ipc.updateProject).toHaveBeenCalledWith(
      "/w",
      "Writer",
      "A calm place to draft essays.",
      "Write for curious beginners.",
    );
    expect(ipc.newSession).not.toHaveBeenCalled();
  });

  it("saves an inline project name with Enter", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });

    const name = screen.getByLabelText("Project name");
    await userEvent.clear(name);
    await userEvent.type(name, "Writer Studio{Enter}");

    expect(ipc.updateProject).toHaveBeenCalledWith(
      "/w",
      "Writer Studio",
      "A calm place to draft essays.",
      "Use plain language.",
    );
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

    await waitFor(() => expect(useStore.getState().projectsOpen).toBe(false));
    expect(ipc.newSession).toHaveBeenCalledOnce();
    const session = useStore.getState().session!;
    expect(useStore.getState().threads[session.session_id]?.[0]).toMatchObject({
      kind: "user",
      text: "Build the landing page",
    });
  });

  it("starts a project chat with the cloud model selected in its composer", async () => {
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
      cloudModels: ipc.sampleCloudModels,
    });
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });

    await userEvent.click(screen.getByRole("button", { name: /claude opus 4\.8/i }));
    await userEvent.click(screen.getByText("Claude Sonnet 4.6"));
    await userEvent.type(screen.getByLabelText("Ask about this project"), "Build the landing page");
    await userEvent.click(screen.getByRole("button", { name: "Send project prompt" }));

    await waitFor(() => expect(ipc.newSession).toHaveBeenCalledOnce());
    expect(ipc.selectCloudModelForNewChats).toHaveBeenCalledWith("claude-sonnet-4-6");
    expect(ipc.setModel).not.toHaveBeenCalled();
  });

  it("uses a selected local model as the project chat's fresh session", async () => {
    useStore.setState({
      session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
      cloudModels: ipc.sampleCloudModels,
    });
    ipc.useLocalModel.mockResolvedValueOnce({
      ...ipc.sampleSession,
      model: "qwen3-8b-q4-k-m",
      session_id: "local-project-session",
      workspace: "/w",
    });
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });

    await userEvent.click(screen.getByRole("button", { name: /claude opus 4\.8/i }));
    await userEvent.click(await screen.findByText("Qwen3 8B · Q4_K_M"));
    await userEvent.type(screen.getByLabelText("Ask about this project"), "Build the landing page");
    await userEvent.click(screen.getByRole("button", { name: "Send project prompt" }));

    await waitFor(() => expect(ipc.useLocalModel).toHaveBeenCalledWith("qwen3-8b-q4-k-m"));
    expect(ipc.newSession).not.toHaveBeenCalled();
    expect(ipc.selectCloudModelForNewChats).not.toHaveBeenCalled();
  });

  it("keeps project home open when a fresh chat cannot be created", async () => {
    ipc.newSession.mockRejectedValueOnce(new Error("agent initialization failed"));
    render(<ProjectsPage />);
    await userEvent.click(screen.getByText("Writer"));
    await screen.findByRole("heading", { name: "Writer" });
    await userEvent.type(screen.getByLabelText("Ask about this project"), "Build the landing page");
    await userEvent.click(screen.getByRole("button", { name: "Send project prompt" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("agent initialization failed");
    expect(useStore.getState().projectsOpen).toBe(true);
    expect(ipc.runTurn).not.toHaveBeenCalled();
  });

  it("is a non-dismissible navigation root", async () => {
    render(<ProjectsPage />);
    await userEvent.keyboard("{Escape}");
    expect(useStore.getState().projectsOpen).toBe(true);
    expect(screen.queryByRole("button", { name: /back to chat/i })).toBeNull();
  });

  it("orders projects by recent activity and can switch to alphabetical", async () => {
    localStorage.clear();
    useStore.setState({
      projects: [
        { ...projects[1], last_used_at: null },
        { ...projects[0], last_used_at: 1_800_000_000 },
        { ...projects[1], path: "/archive", name: "Archive", last_used_at: 1_700_000_000 },
      ],
    });
    render(<ProjectsPage />);
    const cardNames = () =>
      Array.from(document.querySelectorAll(".project-card-name")).map(
        (el) => el.textContent?.replace("current", "").trim(),
      );

    // Recent is the default: newest activity first, never-used projects last.
    expect(cardNames()).toEqual(["Writer", "Archive", "Elsewhere"]);

    await userEvent.click(screen.getByRole("button", { name: /name/i }));
    expect(cardNames()).toEqual(["Archive", "Elsewhere", "Writer"]);

    // The choice survives leaving and reopening the page.
    expect(localStorage.getItem("oxen-harness.projects-sort")).toBe("name");
  });

  it("shows when each project was last used", () => {
    useStore.setState({
      projects: [{ ...projects[0], last_used_at: Math.floor(Date.now() / 1000) - 120 }, projects[1]],
    });
    render(<ProjectsPage />);
    expect(screen.getByText("2m ago")).toBeInTheDocument();
  });

  it("shows a running indicator on projects with mid-run chats", () => {
    useStore.setState({ runStatus: { e1: "running" } });
    render(<ProjectsPage />);
    expect(screen.getByText("Elsewhere").closest(".project-card")?.querySelector(".project-card-running")).not.toBeNull();
    expect(screen.getByText("Writer").closest(".project-card")?.querySelector(".project-card-running")).toBeNull();
  });

  it("removes a project only after confirmation, keeping files and history", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByRole("button", { name: "Remove project: Elsewhere" }));

    // A confirmation modal explains that nothing on disk is deleted; until
    // confirmed, no removal happens.
    expect(await screen.findByText("Remove project?")).toBeInTheDocument();
    expect(ipc.deleteProject).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(ipc.deleteProject).toHaveBeenCalledWith("/elsewhere");
  });

  it("does not remove a project when the confirmation is cancelled", async () => {
    render(<ProjectsPage />);
    await userEvent.click(screen.getByRole("button", { name: "Remove project: Elsewhere" }));
    await screen.findByText("Remove project?");

    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipc.deleteProject).not.toHaveBeenCalled();
  });
});
