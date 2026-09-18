import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { HistoryModal } from "./HistoryModal";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { Project, SessionSummary } from "../../lib/types";

const info = (id: string, workspace = "/w") => ({ ...ipc.sampleSession, session_id: id, workspace });
const sessions: SessionSummary[] = [
  { id: "s1", workspace: "/w", model: "anthropic/claude-sonnet-4-5-20250929", created_at: 1_700_000_000, title: "Fix the parser", message_count: 4, review_status: "", source: "" },
  { id: "s2", workspace: "/w", model: "deepseek/deepseek-v4", created_at: 1_700_000_000, title: "Refactor tokens", message_count: 2, review_status: "", source: "" },
  { id: "other", workspace: "/elsewhere", model: "m", created_at: 1_700_000_000, title: "Other project chat", message_count: 1, review_status: "", source: "" },
  // Imported transcripts are review-only: never listed here.
  { id: "imp", workspace: "/w", model: "m", created_at: 1_700_000_000, title: "Imported chat", message_count: 1, review_status: "", source: "claude-code" },
];
const projects: Project[] = [
  { path: "/w", name: "w", description: "", instructions: "", context: [], remote_repo: null, session_count: 2, active: true, last_used_at: null },
  { path: "/elsewhere", name: "elsewhere", description: "", instructions: "", context: [], remote_repo: null, session_count: 1, active: false, last_used_at: null },
];
const rowOf = (title: string) => screen.getByText(title).closest(".history-item")!;
const search = () => screen.getByRole("searchbox", { name: "Search chats" });

beforeEach(() => {
  resetAll();
  ipc.resumeSession.mockImplementation(async (id = "") => ({ info: info(id), messages: [], running: false }));
  useStore.setState({
    homeOpen: false,
    historyOpen: true,
    session: info("s1"),
    sessions,
    projects,
    chatTabs: { "/w": ["s1"] },
  });
});

describe("HistoryModal", () => {
  it("lists this project's chats, marking the one already open as a tab", () => {
    render(<HistoryModal />);
    expect(screen.getByText("Fix the parser")).toBeInTheDocument();
    expect(screen.getByText("Refactor tokens")).toBeInTheDocument();
    expect(screen.queryByText("Other project chat")).toBeNull();
    expect(screen.queryByText("Imported chat")).toBeNull();
    expect(rowOf("Fix the parser").querySelector(".history-tab-mark")).not.toBeNull();
    expect(rowOf("Refactor tokens").querySelector(".history-tab-mark")).toBeNull();
    // The search box has the keyboard from the start.
    expect(search()).toHaveFocus();
  });

  it("All projects widens the list and names each chat's project", async () => {
    render(<HistoryModal />);
    await userEvent.click(screen.getByRole("radio", { name: "All projects" }));
    expect(screen.getByText("Other project chat")).toBeInTheDocument();
    expect(rowOf("Other project chat").querySelector(".history-project")).toHaveTextContent("elsewhere");
    await userEvent.click(screen.getByRole("radio", { name: "w" }));
    expect(screen.queryByText("Other project chat")).toBeNull();
  });

  it("search narrows by title or model, and says when nothing matches", async () => {
    render(<HistoryModal />);
    await userEvent.type(search(), "refactor");
    expect(screen.queryByText("Fix the parser")).toBeNull();
    expect(screen.getByText("Refactor tokens")).toBeInTheDocument();

    await userEvent.clear(search());
    await userEvent.type(search(), "sonnet");
    expect(screen.getByText("Fix the parser")).toBeInTheDocument();
    expect(screen.queryByText("Refactor tokens")).toBeNull();

    await userEvent.clear(search());
    await userEvent.type(search(), "nothing like this");
    expect(screen.getByText(/Nothing matches/)).toBeInTheDocument();
  });

  it("opening a row makes it the visible tab and closes the history", async () => {
    render(<HistoryModal />);
    await userEvent.click(screen.getByText("Refactor tokens"));
    expect(useStore.getState().historyOpen).toBe(false);
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s2"));
    expect(useStore.getState().chatTabs["/w"]).toEqual(["s1", "s2"]);
  });

  it("arrow keys move the highlight, Enter opens it, Escape closes", async () => {
    render(<HistoryModal />);
    expect(rowOf("Fix the parser")).toHaveClass("highlighted");
    fireEvent.keyDown(search(), { key: "ArrowDown" });
    expect(rowOf("Refactor tokens")).toHaveClass("highlighted");
    fireEvent.keyDown(search(), { key: "ArrowDown" }); // clamps at the end
    expect(rowOf("Refactor tokens")).toHaveClass("highlighted");
    fireEvent.keyDown(search(), { key: "Enter" });
    await waitFor(() => expect(ipc.resumeSession).toHaveBeenCalledWith("s2"));

    useStore.setState({ historyOpen: true });
    fireEvent.keyDown(search(), { key: "Escape" });
    expect(useStore.getState().historyOpen).toBe(false);
  });

  it("Escape closes from anywhere in the modal, not only the search box", () => {
    render(<HistoryModal />);
    fireEvent.keyDown(screen.getByRole("radio", { name: "All projects" }), { key: "Escape" });
    expect(useStore.getState().historyOpen).toBe(false);
  });

  it("a chat that won't open leaves a notice", async () => {
    ipc.resumeSession.mockRejectedValueOnce(new Error("gone"));
    render(<HistoryModal />);
    await userEvent.click(screen.getByText("Refactor tokens"));
    await waitFor(() =>
      expect(useStore.getState().threads.s1?.some((i) => i.kind === "notice" && /gone/.test(i.text))).toBe(true),
    );
  });

  it("picked from Home, rides out to the chat", async () => {
    useStore.setState({ homeOpen: true });
    render(<HistoryModal />);
    await userEvent.click(screen.getByText("Refactor tokens"));
    await waitFor(() => expect(useStore.getState().homeOpen).toBe(false));
  });

  it("puts loose ends first, each with its reason and the check dot", () => {
    useStore.setState({ runStatus: { s2: "unread" } });
    render(<HistoryModal />);
    const head = screen.getByText("Needs you").closest(".history-head")!;
    expect(head.querySelector(".history-count")).toHaveTextContent("1");
    const titles = Array.from(document.querySelectorAll(".history-title")).map((el) => el.textContent);
    expect(titles).toEqual(["Refactor tokens", "Fix the parser"]);
    expect(rowOf("Refactor tokens").querySelector(".history-need")).toHaveTextContent("finished while you were away");
    expect(rowOf("Refactor tokens").querySelector(".status-dot.check")).not.toBeNull();
    expect(screen.getByText("Other chats")).toBeInTheDocument();
  });

  it("shows the model and date for each chat", () => {
    render(<HistoryModal />);
    expect(rowOf("Fix the parser").querySelector(".history-model")).toHaveTextContent("claude-sonnet-4-5");
    expect(rowOf("Fix the parser").querySelector(".history-date")).not.toBeNull();
  });

  it("deletes a chat only after confirming", async () => {
    render(<HistoryModal />);
    await userEvent.click(screen.getByRole("button", { name: "Delete chat: Refactor tokens" }));
    expect(ipc.deleteSession).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(ipc.deleteSession).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Delete chat: Refactor tokens" }));
    await userEvent.click(screen.getByRole("button", { name: /^delete$/i }));
    expect(ipc.deleteSession).toHaveBeenCalledWith("s2");
  });

  it("fetches the board once when nothing has, so rows still section", () => {
    render(<HistoryModal />);
    expect(ipc.ledgerSnapshot).toHaveBeenCalledOnce();
  });
});
