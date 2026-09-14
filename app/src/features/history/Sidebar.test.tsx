import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Sidebar } from "./Sidebar";
import { ProjectsNav } from "../projects/ProjectsNav";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { LedgerEntry, Project, SessionSummary } from "../../lib/types";

const sessions: SessionSummary[] = [
  { id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "First chat", message_count: 4, review_status: "", source: "" },
  { id: "s2", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Second chat", message_count: 2, review_status: "", source: "" },
  { id: "other", workspace: "/elsewhere", model: "m", created_at: 1_700_000_000, title: "Other project chat", message_count: 1, review_status: "", source: "" },
];
const projects: Project[] = [
  { path: "/w", name: "w", description: "", instructions: "", context: [], remote_repo: null, session_count: 2, active: true, last_used_at: null },
  { path: "/elsewhere", name: "elsewhere", description: "", instructions: "", context: [], remote_repo: null, session_count: 1, active: false, last_used_at: null },
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

  it("pins a brand-new active chat on top until its first message lands", async () => {
    // A fresh chat has no user turn, so the persisted list can't carry it —
    // without the pin the open chat would have no row and no highlight.
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "fresh", workspace: "/w" } });
    render(<Sidebar />);
    const row = screen
      .getByText("New chat", { selector: ".history-title" })
      .closest(".history-item")!;
    expect(row).toHaveClass("active");

    // Pressing "+" while already on the fresh chat stays put — no orphan
    // empty sessions pile up in the store.
    await userEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(ipc.newSession).not.toHaveBeenCalled();
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

  // The home back-link is column chrome now (above the Chats/Files tabs),
  // not part of the Sidebar — see ProjectsNav.
  it("opens Home from the nav", async () => {
    render(<ProjectsNav />);
    await userEvent.click(screen.getByRole("button", { name: /^home/i }));
    expect(useStore.getState().homeOpen).toBe(true);
  });

  it("signals activity in other projects on the Projects nav", () => {
    useStore.setState({ runStatus: { other: "running" } });
    render(<ProjectsNav />);
    expect(document.querySelector(".projects-nav-dot")).not.toBeNull();
  });

  it("shows no activity dot when only the current project is busy", () => {
    useStore.setState({ runStatus: { s2: "running" } });
    render(<ProjectsNav />);
    expect(document.querySelector(".projects-nav-dot")).toBeNull();
  });

  it("opens Settings from the footer button", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByRole("button", { name: /settings/i }));
    expect(useStore.getState().settingsOpen).toBe(true);
  });

  it("shows a running indicator, and an unread finish wears the needs-you dot", () => {
    // Unread used to be its own quiet dot; a finish the user hasn't looked at
    // is a loose end now, so it takes the warning dot and the Needs you band.
    useStore.setState({ runStatus: { s1: "running", s2: "unread" } });
    render(<Sidebar />);
    const first = screen.getByText("First chat").closest(".history-item")!;
    const second = screen.getByText("Second chat").closest(".history-item")!;
    expect(first.querySelector(".chat-status.running")).not.toBeNull();
    expect(second.querySelector(".chat-status.needy")).not.toBeNull();
    expect(second.querySelector(".chat-status.unread")).toBeNull();
  });

  it("sections the list into Needs you / other chats / settled, with the reason on each loose end", () => {
    const now = Math.floor(Date.now() / 1000);
    const ledgerEntry = (overrides: Partial<LedgerEntry>): LedgerEntry => ({
      id: "s1",
      workspace: "/w",
      model: "m",
      created_at: now - 86_400,
      last_activity_at: now - 3_600,
      title: "",
      last_reply: "",
      message_count: 2,
      mid_turn: false,
      plan: null,
      trail: null,
      settle: null,
      review_status: "",
      seen_at: 0,
      ...overrides,
    });
    useStore.setState({
      sessions: [
        ...sessions,
        { id: "s3", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Third chat", message_count: 2, review_status: "", source: "" },
        { id: "s4", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Fourth chat", message_count: 2, review_status: "", source: "" },
      ],
      ledger: {
        entries: [
          // s1: an ordinary open chat — the last word was spoken, nothing owed.
          ledgerEntry({ id: "s1", title: "First chat" }),
          // s2: the reply never arrived.
          ledgerEntry({ id: "s2", title: "Second chat", mid_turn: true }),
          // s3: tied off.
          ledgerEntry({ id: "s3", title: "Third chat", settle: { settled_at: now - 60, note: "" } }),
          // s4: finished after the user last looked at it.
          ledgerEntry({ id: "s4", title: "Fourth chat", seen_at: now - 7_200 }),
        ],
        running: [],
        last_seen: now - 60,
      },
    });
    render(<Sidebar />);

    // The band headers, with the loose-end count the home card promised.
    const needsHead = screen.getByText("Needs you").closest(".history-head")!;
    expect(needsHead.querySelector(".history-count")).toHaveTextContent("2");
    expect(screen.getByText("Other chats")).toBeInTheDocument();
    expect(screen.getByText("Settled")).toBeInTheDocument();

    // Rows land in the right bands, most urgent first, each wearing its reason.
    const titles = Array.from(document.querySelectorAll(".history-title")).map((el) => el.textContent);
    expect(titles).toEqual(["Second chat", "Fourth chat", "First chat", "Third chat"]);
    const second = screen.getByText("Second chat").closest(".history-item")!;
    expect(second).toHaveClass("needy");
    expect(second.querySelector(".history-need")).toHaveTextContent(/left dangling/);
    expect(second.querySelector(".chat-status.needy")).not.toBeNull();
    const fourth = screen.getByText("Fourth chat").closest(".history-item")!;
    expect(fourth.querySelector(".history-need")).toHaveTextContent("finished while you were away");
    expect(screen.getByText("First chat").closest(".history-item")!.querySelector(".history-need")).toBeNull();
    expect(screen.getByText("Third chat").closest(".history-item")).toHaveClass("settled");
  });

  it("a finish the store saw land offscreen counts as needing you before the board catches up", () => {
    useStore.setState({ runStatus: { s2: "unread" } });
    render(<Sidebar />);
    expect(screen.getByText("Needs you")).toBeInTheDocument();
    const second = screen.getByText("Second chat").closest(".history-item")!;
    expect(second.querySelector(".history-need")).toHaveTextContent("finished while you were away");
  });

  it("fetches the board once when nothing has, so a chat reached without visiting Home still sections", () => {
    render(<Sidebar />);
    expect(ipc.ledgerSnapshot).toHaveBeenCalledOnce();
  });

  it("shows the model and date for each chat", () => {
    useStore.setState({
      sessions: [
        { id: "s1", workspace: "/w", model: "anthropic/claude-sonnet-4-5-20250929", created_at: 1_700_000_000, title: "First chat", message_count: 4, review_status: "", source: "" },
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
