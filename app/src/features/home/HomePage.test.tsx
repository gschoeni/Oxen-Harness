import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { HomePage } from "./HomePage";
import { useStore } from "../../lib/store";
import { getUi, setUi } from "../../lib/uiState";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";
import type { Project, ThreadEntry, ThreadSnapshot } from "../../lib/types";

const NOW = Math.floor(Date.now() / 1000);
const DAY = 86_400;

function entry(overrides: Partial<ThreadEntry> = {}): ThreadEntry {
  return {
    id: "s1",
    workspace: "/work/app",
    model: "m",
    created_at: NOW - DAY,
    last_activity_at: NOW - 3_600,
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

function project(path: string, name: string): Project {
  return {
    path,
    name,
    description: "",
    instructions: "",
    context: [],
    remote_repo: null,
    session_count: 1,
    active: false,
    last_used_at: NOW - DAY,
  };
}

function seed(snapshot: Partial<ThreadSnapshot>, projects: Project[] = []) {
  const full = { entries: [], running: [], ...snapshot };
  useStore.setState({ threadsSnapshot: full, projects });
  // Home refreshes itself on mount; the backend must tell the same story or
  // the refresh would silently wipe the seeded state mid-test.
  vi.mocked(ipc.threadsSnapshot).mockResolvedValue(full);
}

beforeEach(() => {
  resetAll();
});

describe("Home", () => {
  it("opens on the cards, one per project, wearing its vital signs", () => {
    setUi("homeView", undefined); // a fresh install has no lens habit yet
    seed(
      {
        entries: [
          entry({ id: "a", title: "revamp home" }),
          entry({ id: "b", title: "busy one", mid_turn: true }),
          entry({ id: "c", title: "dropped one", mid_turn: true }),
        ],
        running: ["b"],
      },
      [project("/work/app", "App")],
    );
    const { container } = render(<HomePage />);
    // The run dot for "b", the attention pill for dangling "c", all three open.
    expect(screen.getByText("App")).toBeTruthy();
    expect(container.querySelector(".project-card-running .run-dot")).toBeTruthy();
    expect(screen.getByText("1 needs you")).toHaveClass("project-card-attention");
    expect(screen.getByText("3 open")).toBeTruthy();
  });

  it("a stuck agent counts toward the card's attention pill", () => {
    seed(
      {
        entries: [
          entry({ id: "parked", title: "waiting on approval", mid_turn: true }),
          entry({ id: "calm", title: "cruising", mid_turn: true }),
        ],
        running: ["parked", "calm"],
      },
      [project("/work/app", "App")],
    );
    // A running thread needs nothing — unless it's parked on an approval,
    // which is the loudest claim on attention there is.
    useStore.setState({
      approvals: { parked: { id: "a1", session: "parked" } as never },
    });
    render(<HomePage />);
    expect(screen.getByText("1 needs you")).toHaveClass("project-card-attention");
  });

  it("a finished thread is neither open nor needy on the card", () => {
    seed(
      { entries: [entry({ id: "done", finished_at: NOW - 60 }), entry({ id: "open" })] },
      [project("/work/app", "App")],
    );
    render(<HomePage />);
    expect(screen.getByText("1 open")).toBeTruthy();
    expect(screen.queryByText(/needs? you/)).toBeNull();
  });

  it("a card resumes the project's newest chat and leaves Home", async () => {
    seed({ entries: [entry({ id: "s1" })] }, [project("/work/app", "App")]);
    useStore.setState({
      sessions: [
        {
          id: "s1",
          workspace: "/work/app",
          model: "m",
          created_at: NOW - DAY,
          title: "fix the flaky test",
          message_count: 8,
          review_status: "",
          source: "",
        },
      ],
    });
    const { container } = render(<HomePage />);
    await userEvent.click(container.querySelector(".project-card-open") as HTMLElement);
    await waitFor(() => expect(ipc.resumeSession).toHaveBeenCalledWith("s1"));
    expect(useStore.getState().homeOpen).toBe(false);
  });

  it("offers the first project when there is nothing at all", () => {
    seed({ entries: [] });
    render(<HomePage />);
    expect(screen.getByText("Start your first project")).toBeTruthy();
  });

  it("has a media lens that lists projects' generations and sticks", async () => {
    const app = project("/work/app", "app");
    seed({ entries: [entry()] }, [app]);
    vi.mocked(ipc.listMedia).mockResolvedValue([
      {
        id: "g1",
        session: "s1",
        turn_seq: null,
        call_id: null,
        batch: "b",
        index: 1,
        kind: "image",
        model: "flux",
        prompt: "an ox",
        params: {},
        refs: [],
        path: "generations/a.png",
        poster: null,
        bytes: 1,
        width: 4,
        height: 2,
        duration_secs: null,
        cost_usd: 0.01,
        status: "succeeded",
        error: null,
        created_at: NOW,
        completed_at: null,
        parent: null,
        seed: null,
        sources: [],
        agent_prompt: null,
        provider: null,
      },
    ]);
    render(<HomePage />);
    await userEvent.click(await screen.findByRole("button", { name: /media/i }));
    expect(getUi("homeView")).toBe("media");
    expect(await screen.findByTitle("an ox")).toBeInTheDocument();
    expect(screen.getByText("1 generation")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /cards/i }));
    expect(getUi("homeView")).toBe("cards");
    expect(screen.getByText("app")).toBeTruthy();
  });

  it("follows store navigation while mounted — the titlebar buttons are never dead", async () => {
    seed({ entries: [entry({ title: "thread a" })] }, [project("/work/app", "App")]);
    render(<HomePage />);

    // Chrome outside Home targets a project page (e.g. the titlebar's
    // project button) while Home is already up.
    useStore.getState().openProjectHome("/work/app");
    await waitFor(() => expect(screen.getByLabelText("Project name")).toBeTruthy());

    // And the titlebar's "N running — go Home" chip goes back from that
    // page. Before the store drove this, the page snapshot was mount-only
    // and both buttons looked dead.
    useStore.getState().setHomeOpen(true);
    await waitFor(() => expect(screen.getByText("Home")).toBeTruthy());
    expect(screen.queryByLabelText("Project name")).toBeNull();
  });

  it("removes a project from its own page, corner trash behind a confirm", async () => {
    seed({ entries: [entry({ title: "thread a" })] }, [project("/work/app", "App")]);
    render(<HomePage />);
    useStore.getState().openProjectHome("/work/app");
    await waitFor(() => expect(screen.getByLabelText("Project name")).toBeTruthy());

    await userEvent.click(screen.getByRole("button", { name: "Remove project: App" }));
    expect(ipc.deleteProject).not.toHaveBeenCalled();

    // The backend forgets the project's threads too (removed workspaces drop
    // out of the snapshot) — the next refresh must reflect that.
    vi.mocked(ipc.threadsSnapshot).mockResolvedValue({ entries: [], running: [] });
    vi.mocked(ipc.listProjects).mockResolvedValue([]);

    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(ipc.deleteProject).toHaveBeenCalledWith("/work/app"));
    // The project is gone — we land back on Home without its card.
    await waitFor(() => expect(screen.getByText("Home")).toBeTruthy());
    await waitFor(() => expect(screen.queryByText("App")).toBeNull());
  });
});
