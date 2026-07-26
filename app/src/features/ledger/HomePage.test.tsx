import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { HomePage } from "./HomePage";
import { useStore } from "../../lib/store";
import { getUi, setUi } from "../../lib/uiState";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";
import type { LedgerEntry, LedgerSnapshot, Project } from "../../lib/types";

const NOW = Math.floor(Date.now() / 1000);
const DAY = 86_400;

function entry(overrides: Partial<LedgerEntry> = {}): LedgerEntry {
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
    plan: null,
    trail: null,
    settle: null,
    review_status: "",
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
    session_count: 1,
    active: false,
    last_used_at: NOW - DAY,
  };
}

function seed(snapshot: Partial<LedgerSnapshot>, projects: Project[] = []) {
  const full = { entries: [], running: [], last_seen: 0, ...snapshot };
  useStore.setState({ ledger: full, projects });
  // The board refreshes itself on mount; the backend must tell the same story
  // or the refresh would silently wipe the seeded state mid-test.
  vi.mocked(ipc.ledgerSnapshot).mockResolvedValue(full);
}

beforeEach(() => {
  resetAll();
});

/** Unfold the wagon row wearing this title. The header's pick-up button and
 *  the rail can wear the same words, so role+name alone is ambiguous. */
function clickWagon(container: HTMLElement, title: RegExp) {
  const row = [...container.querySelectorAll<HTMLElement>(".ledger-wagon")].find((el) =>
    title.test(el.textContent ?? ""),
  );
  return userEvent.click(row as HTMLElement);
}


describe("the board", () => {
  it("groups threads into wagon trains with git banners and statuses", () => {
    seed(
      {
        entries: [
          entry({ id: "a", title: "revamp home screen", plan: { done: 3, total: 5, active: "Styling" } }),
          entry({ id: "b", title: "fix sse test", workspace: "/work/blog" }),
        ],
        running: ["a"],
      },
      [project("/work/app", "App"), project("/work/blog", "Blog")],
    );
    useStore.setState({
      ledgerGit: {
        "/work/app": { branch: "main", dirty_files: 2, ahead: 1, behind: 0, has_upstream: true },
      },
    });

    render(<HomePage />);
    expect(screen.getByRole("button", { name: "App" })).toBeTruthy();
    expect(screen.getByText("main")).toBeTruthy();
    expect(screen.getByText("±2")).toBeTruthy();
    expect(screen.getByText("↑1")).toBeTruthy();
    expect(screen.getByText("riding · 3/5")).toBeTruthy();
    expect(screen.getByText(/done ·/)).toBeTruthy();
  });

  it("a dangling thread wears its loose end right on its train row", () => {
    seed({ entries: [entry({ id: "d", title: "refactor auth", mid_turn: true })] });
    render(<HomePage />);
    expect(screen.getByText(/left dangling ·/)).toBeTruthy();
  });

  it("tells the since-you-left story when something finished while away", () => {
    seed({
      entries: [entry({ last_activity_at: NOW - 60 })],
      last_seen: NOW - 14 * 3_600,
    });
    render(<HomePage />);
    expect(screen.getByText(/away 14h/)).toBeTruthy();
    expect(screen.getByText(/1 while you were out/)).toBeTruthy();
  });

  it("unfolds a waystation, then opens the thread and rides out", async () => {
    seed({
      entries: [
        entry({
          id: "s9",
          title: "dark mode css",
          last_reply: "Swapped the palette to tokens; both themes pass contrast.",
        }),
      ],
    });
    const { container } = render(<HomePage />);

    // First click unfolds the ledger row — the board is still up.
    await clickWagon(container, /dark mode css/);
    expect(screen.getByText(/both themes pass contrast/)).toBeTruthy();
    expect(useStore.getState().homeOpen).toBe(true);

    await userEvent.click(screen.getByRole("button", { name: "Open chat" }));
    await waitFor(() => expect(useStore.getState().homeOpen).toBe(false));
    expect(ipc.resumeSession).toHaveBeenCalledWith("s9");
    // Riding out records the visit, so next time "since you left" is honest.
    expect(ipc.ledgerMarkSeen).toHaveBeenCalled();
  });

  it("ties off a thread in one click — no note, no extra step", async () => {
    seed({ entries: [entry({ id: "s3", title: "bump deps" })] });
    const { container } = render(<HomePage />);

    await clickWagon(container, /bump deps/);
    expect(screen.queryByPlaceholderText(/closing note/)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /tie the knot/i }));

    // The knot ritual plays first; the settle write lands when it's done.
    await waitFor(() => expect(ipc.settleSession).toHaveBeenCalledWith("s3", ""), {
      timeout: 2_000,
    });
    expect(ipc.ledgerSnapshot).toHaveBeenCalled();
  });

  it("never offers tie-off on a running thread", async () => {
    seed({ entries: [entry({ id: "r", title: "long migration" })], running: ["r"] });
    const { container } = render(<HomePage />);
    await clickWagon(container, /long migration/);
    expect(screen.queryByRole("button", { name: /tie the knot/i })).toBeNull();
    expect(screen.getByRole("button", { name: "Open chat" })).toBeTruthy();
  });

  it("a dangling thread's waystation says so and offers to pick it back up", async () => {
    seed({ entries: [entry({ id: "d", title: "refactor auth", mid_turn: true })] });
    const { container } = render(<HomePage />);
    // The thread is on the rail too — unfold specifically its wagon row.
    await userEvent.click(container.querySelector(".ledger-wagon") as HTMLElement);
    expect(screen.getByText("the reply never arrived")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Pick it back up" }));
    await waitFor(() => expect(ipc.resumeSession).toHaveBeenCalledWith("d"));
  });

  it("tallies settled threads and unties one on request", async () => {
    seed({
      entries: [
        entry({
          id: "tied",
          title: "bump deps",
          settle: { settled_at: NOW - 120, note: "shipped as PR #42" },
        }),
        entry({ id: "open", title: "still open" }),
      ],
    });
    render(<HomePage />);
    await userEvent.click(screen.getByRole("button", { name: /settled/i }));
    expect(screen.getByText(/shipped as PR #42/)).toBeTruthy();
    await userEvent.click(screen.getByTitle(/untie/i));
    await waitFor(() => expect(ipc.reopenSession).toHaveBeenCalledWith("tied"));
    expect(ipc.ledgerSnapshot).toHaveBeenCalled();
  });

  it("a running wagon row grows a live tool readout; idle rows stay flat", () => {
    seed(
      {
        entries: [
          entry({ id: "r1", title: "revamp home" }),
          entry({ id: "r2", title: "fix flaky test", workspace: "/work/blog" }),
          entry({ id: "idle", title: "quiet one" }),
        ],
        running: ["r1", "r2"],
      },
      [project("/work/app", "App"), project("/work/blog", "Blog")],
    );
    useStore.setState({
      trailActivity: {
        r1: { name: "edit_file", detail: "src/features/ledger/HomePage.tsx", at: 1 },
      },
    });

    const { container } = render(<HomePage />);
    const live = [...container.querySelectorAll(".ledger-wagon-live")].map((el) => el.textContent);
    // One readout per RUNNING thread: the mid-tool one and the thinking one.
    expect(live).toHaveLength(2);
    expect(live.join(" ")).toContain("⚙ edit_file");
    expect(live.join(" ")).toContain("HomePage.tsx");
    expect(live.join(" ")).toContain("thinking");
  });

  it("purges the archive: select all, one confirm, every chat deleted", async () => {
    seed({
      entries: [
        entry({ id: "l1", title: "old spike", last_activity_at: NOW - 20 * DAY }),
        entry({ id: "l2", title: "older spike", last_activity_at: NOW - 30 * DAY }),
        entry({ id: "keep", title: "still warm" }),
      ],
    });
    render(<HomePage />);
    await userEvent.click(screen.getByRole("button", { name: /lost to the trail/i }));

    await userEvent.click(screen.getByRole("button", { name: "select all" }));
    await userEvent.click(screen.getByRole("button", { name: /delete 2/i }));
    expect(ipc.deleteSession).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Delete 2" }));
    await waitFor(() => expect(ipc.deleteSession).toHaveBeenCalledTimes(2));
    expect(ipc.deleteSession).toHaveBeenCalledWith("l1");
    expect(ipc.deleteSession).toHaveBeenCalledWith("l2");
    // One refresh at the end, not one per chat.
    expect(ipc.ledgerSnapshot).toHaveBeenCalled();
  });

  it("deletes a single archived chat from its row", async () => {
    seed({
      entries: [entry({ id: "l1", title: "old spike", last_activity_at: NOW - 20 * DAY })],
    });
    render(<HomePage />);
    await userEvent.click(screen.getByRole("button", { name: /lost to the trail/i }));
    await userEvent.click(screen.getByLabelText("Delete chat: old spike"));
    await userEvent.click(screen.getByRole("button", { name: "Delete chat" }));
    await waitFor(() => expect(ipc.deleteSession).toHaveBeenCalledWith("l1"));
  });

  it("banishes cold threads to the archive, browsable and rekindlable", async () => {
    seed({
      entries: [
        entry({ id: "cold", title: "ancient refactor", last_activity_at: NOW - 20 * DAY }),
      ],
    });
    render(<HomePage />);
    // Not on the board proper…
    expect(screen.queryByText(/ancient refactor/)).toBeNull();
    // …but waiting in the archive.
    await userEvent.click(screen.getByRole("button", { name: /lost to the trail/i }));
    expect(screen.getByText("ancient refactor")).toBeTruthy();
    await userEvent.click(screen.getByTitle(/rekindle/i));
    await waitFor(() => expect(ipc.resumeSession).toHaveBeenCalledWith("cold"));
  });

  it("collapses projects with nothing on the trail to a quiet line", () => {
    seed({ entries: [] }, [project("/work/dotfiles", "dotfiles")]);
    render(<HomePage />);
    expect(screen.getByText("dotfiles")).toBeTruthy();
    expect(screen.getByText(/quiet ·/)).toBeTruthy();
  });

  it("offers the first trail when there is nothing at all", () => {
    seed({ entries: [] });
    render(<HomePage />);
    expect(screen.getByText("Open your first trail")).toBeTruthy();
  });

  it("wears the model's charted title and stage, with named stations on the line", () => {
    seed({
      entries: [
        entry({
          id: "c",
          title: "hey can you look at the flaky test",
          trail: {
            title: "fix flaky sse retry test",
            waypoints: [
              { name: "define", status: "done" },
              { name: "implement", status: "current" },
              { name: "review", status: "ahead" },
            ],
          },
        }),
      ],
    });
    const { container } = render(<HomePage />);
    // The wagon row wears the charted title (the pick-up button does too).
    expect(container.querySelector(".ledger-wagon-title")?.textContent).toContain(
      "fix flaky sse retry test",
    );
    expect(screen.queryByText(/hey can you look/)).toBeNull();
    expect(screen.getByText(/implement ·/)).toBeTruthy();
    expect(container.querySelectorAll(".trail-station")).toHaveLength(3);
    expect(container.querySelectorAll(".trail-station.done")).toHaveLength(1);
  });

  it("caps a train at three wagons and rides the rest into the project page", async () => {
    seed(
      {
        entries: ["a", "b", "c", "d", "e"].map((id, i) =>
          entry({ id, title: `thread ${id}`, last_activity_at: NOW - i * 100 }),
        ),
      },
      [project("/work/app", "App")],
    );
    const { container } = render(<HomePage />);
    expect(container.querySelectorAll(".ledger-wagon")).toHaveLength(3);
    const more = screen.getByRole("button", { name: /and 2 more on this trail/ });
    await userEvent.click(more);
    // The project home opens (its trail carries the full list).
    await waitFor(() => expect(screen.getByLabelText("Project name")).toBeTruthy());
  });

  it("keeps curation out of the waystation — that lives in the chat's Inspector", async () => {
    seed({ entries: [entry({ id: "k", title: "bump deps" })] });
    const { container } = render(<HomePage />);
    await clickWagon(container, /bump deps/);
    expect(screen.queryByRole("button", { name: /keep/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /reject/i })).toBeNull();
  });

  it("removes a project from its own page, corner trash behind a confirm", async () => {
    seed(
      {
        entries: ["a", "b", "c", "d"].map((id, i) =>
          entry({ id, title: `thread ${id}`, last_activity_at: NOW - i * 100 }),
        ),
      },
      [project("/work/app", "App")],
    );
    render(<HomePage />);
    // Ride into the project page via the train's overflow row.
    await userEvent.click(screen.getByRole("button", { name: /more on this trail/ }));
    await waitFor(() => expect(screen.getByLabelText("Project name")).toBeTruthy());

    await userEvent.click(screen.getByRole("button", { name: "Remove project: App" }));
    expect(ipc.deleteProject).not.toHaveBeenCalled();

    // The backend forgets the project's threads too (removed workspaces drop
    // out of the snapshot) — the next refresh must reflect that.
    vi.mocked(ipc.ledgerSnapshot).mockResolvedValue({ entries: [], running: [], last_seen: 0 });
    vi.mocked(ipc.listProjects).mockResolvedValue([]);

    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(ipc.deleteProject).toHaveBeenCalledWith("/work/app"));
    // The project is gone — we land back on the board and its train is gone.
    await waitFor(() => expect(screen.getByText("Home")).toBeTruthy());
    await waitFor(() => expect(screen.queryByText(/thread a/)).toBeNull());
  });

  it("toggles between the ledger and cards lenses, and the lens sticks", async () => {
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
    // The ledger lens by default.
    expect(container.querySelector(".ledger-wagon")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: /cards/i }));
    expect(container.querySelector(".ledger-wagon")).toBeNull();
    // One card per project, wearing its vital signs from the board: the run
    // dot for "b", a need for dangling "c", all three on the trail.
    expect(screen.getByText("App")).toBeTruthy();
    expect(container.querySelector(".project-card-running .run-dot")).toBeTruthy();
    expect(screen.getByText("1 need you")).toBeTruthy();
    expect(screen.getByText("3 on the trail")).toBeTruthy();
    expect(getUi("homeView")).toBe("cards");

    await userEvent.click(screen.getByRole("button", { name: /^ledger$/i }));
    expect(container.querySelector(".ledger-wagon")).toBeTruthy();
    expect(getUi("homeView")).toBe("ledger");
  });

  it("a card resumes the project's newest chat and rides out", async () => {
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
    setUi("homeView", "cards");
    const { container } = render(<HomePage />);
    await userEvent.click(container.querySelector(".project-card-open") as HTMLElement);
    await waitFor(() => expect(ipc.resumeSession).toHaveBeenCalledWith("s1"));
    expect(useStore.getState().homeOpen).toBe(false);
  });

  it("cuts a thread loose only after the confirm", async () => {
    seed({ entries: [entry({ id: "gone", title: "old experiment" })] });
    const { container } = render(<HomePage />);
    await clickWagon(container, /old experiment/);
    await userEvent.click(screen.getByLabelText("Delete chat: old experiment"));
    expect(ipc.deleteSession).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Delete chat" }));
    await waitFor(() => expect(ipc.deleteSession).toHaveBeenCalledWith("gone"));
  });
});
