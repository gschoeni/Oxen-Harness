import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ProjectsNav } from "./ProjectsNav";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { SessionSummary } from "../../lib/types";

const sessions: SessionSummary[] = [
  { id: "s1", workspace: "/w", model: "m", created_at: 1, title: "First chat", message_count: 4, review_status: "", source: "" },
  { id: "s2", workspace: "/w", model: "m", created_at: 1, title: "Second chat", message_count: 2, review_status: "", source: "" },
  { id: "other", workspace: "/elsewhere", model: "m", created_at: 1, title: "Other project chat", message_count: 1, review_status: "", source: "" },
];

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
    sessions,
    homeOpen: false,
  });
});

// The home back-link is column chrome above the left dock — home is one
// click from the Files tree.
describe("ProjectsNav", () => {
  it("opens Home from the nav", async () => {
    render(<ProjectsNav />);
    await userEvent.click(screen.getByRole("button", { name: /^home/i }));
    expect(useStore.getState().homeOpen).toBe(true);
  });

  it("signals activity in other projects", () => {
    useStore.setState({ runStatus: { other: "running" } });
    render(<ProjectsNav />);
    expect(document.querySelector(".projects-nav-dot")).not.toBeNull();
  });

  it("shows no activity dot when only the current project is busy", () => {
    useStore.setState({ runStatus: { s2: "running" } });
    render(<ProjectsNav />);
    expect(document.querySelector(".projects-nav-dot")).toBeNull();
  });
});
