import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ChatTabs } from "./ChatTabs";
import { useChatTabShortcuts } from "./shortcuts";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { SessionSummary } from "../../lib/types";

const W = "/w";
const info = (id: string) => ({ ...ipc.sampleSession, session_id: id, workspace: W });
const summary = (id: string, title: string): SessionSummary => ({
  id,
  workspace: W,
  model: "anthropic/claude-sonnet-4-5-20250929",
  created_at: 1_700_000_000,
  title,
  message_count: 2,
  review_status: "",
  source: "",
});
const tab = (title: string) => screen.getByRole("tab", { name: new RegExp(title) });

beforeEach(() => {
  resetAll();
  ipc.resumeSession.mockImplementation(async (id = "") => ({ info: info(id), messages: [], running: false }));
  const sessions = [summary("s1", "Fix the parser"), summary("s2", "Refactor tokens"), summary("s3", "Closed chat")];
  // Every session swap re-reads the history; keep it answering the same list.
  ipc.listSessions.mockResolvedValue(sessions as never);
  useStore.setState({
    homeOpen: false,
    session: info("s1"),
    sessions,
    chatTabs: { [W]: ["s1", "s2"] },
  });
});

describe("ChatTabs", () => {
  it("shows one tab per open chat in this project, the visible one active", () => {
    render(<ChatTabs />);
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["Fix the parser", "Refactor tokens"]);
    expect(tabs[0]).toHaveAttribute("aria-selected", "true");
    expect(tabs[1]).toHaveAttribute("aria-selected", "false");
    // A chat with no tab isn't in the strip — it's in the history.
    expect(screen.queryByText("Closed chat")).toBeNull();
  });

  it("puts + right after the tabs, outside the scroller, and the history at the end", () => {
    render(<ChatTabs />);
    const strip = screen.getByRole("tablist");
    const plus = screen.getByRole("button", { name: "New chat" });
    const history = screen.getByRole("button", { name: "All chats" });
    expect(strip.contains(plus)).toBe(false);
    expect(strip.nextElementSibling).toBe(plus);
    expect(plus.compareDocumentPosition(history) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("a chat with no turn yet is called New chat", () => {
    useStore.setState({ session: info("fresh"), chatTabs: { [W]: ["s1", "fresh"] } });
    render(<ChatTabs />);
    expect(tab("New chat")).toHaveAttribute("aria-selected", "true");
  });

  it("a fresh chat is named by its first prompt the moment it is sent", () => {
    useStore.setState({
      session: info("fresh"),
      chatTabs: { [W]: ["s1", "fresh"] },
      threads: { fresh: [{ id: "u1", kind: "user", text: "  Port the parser to Rust\nwith tests " }] },
    });
    render(<ChatTabs />);
    expect(tab("Port the parser to Rust")).toHaveAttribute("aria-selected", "true");
    // Once the history lists it (clipped, or renamed), that title wins.
    act(() => useStore.setState({ sessions: [summary("fresh", "Parser port")] }));
    expect(tab("Parser port")).toBeInTheDocument();
  });

  it("double-click renames a tab in place; Enter keeps the name, Escape drops it", async () => {
    render(<ChatTabs />);
    await userEvent.dblClick(tab("Refactor tokens"));
    const field = screen.getByRole("textbox", { name: "Chat name" });
    expect(field).toHaveValue("Refactor tokens");
    await userEvent.clear(field);
    // The list the rename re-reads afterwards carries the new name too.
    ipc.listSessions.mockResolvedValue([
      summary("s1", "Fix the parser"),
      summary("s2", "Tokens v2"),
      summary("s3", "Closed chat"),
    ] as never);
    await userEvent.type(field, "Tokens v2{Enter}");
    expect(ipc.renameSession).toHaveBeenCalledWith("s2", "Tokens v2");
    expect(tab("Tokens v2")).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).toBeNull();

    await userEvent.dblClick(tab("Fix the parser"));
    await userEvent.type(screen.getByRole("textbox", { name: "Chat name" }), " nope{Escape}");
    expect(ipc.renameSession).toHaveBeenCalledOnce();
    expect(tab("Fix the parser")).toBeInTheDocument();
  });

  it("an unchanged or blank edit sends nothing new, and blank clears the name", async () => {
    render(<ChatTabs />);
    await userEvent.dblClick(tab("Refactor tokens"));
    await userEvent.type(screen.getByRole("textbox", { name: "Chat name" }), "{Enter}");
    expect(ipc.renameSession).not.toHaveBeenCalled();

    await userEvent.dblClick(tab("Refactor tokens"));
    await userEvent.clear(screen.getByRole("textbox", { name: "Chat name" }));
    await userEvent.keyboard("{Enter}");
    expect(ipc.renameSession).toHaveBeenCalledWith("s2", "");
  });

  it("Rename… in the menu opens the same editor", async () => {
    render(<ChatTabs />);
    fireEvent.contextMenu(tab("Refactor tokens"));
    await userEvent.click(screen.getByRole("option", { name: "Rename…" }));
    expect(screen.getByRole("textbox", { name: "Chat name" })).toHaveValue("Refactor tokens");
  });

  it("drag a tab onto another to reorder; past the midpoint lands after it", () => {
    useStore.setState({ chatTabs: { [W]: ["s1", "s2", "s3"] } });
    render(<ChatTabs />);
    const dt = () => ({ types: ["application/x-oxen-chat-tab"], setData: () => {}, getData: () => "s3", effectAllowed: "", dropEffect: "" });
    // jsdom has no DragEvent, so a dragover with a pointer position is a
    // MouseEvent carrying the transfer; rects are all zero, so any positive
    // x is "past the midpoint".
    const dragOverAt = (el: HTMLElement, clientX: number) => {
      const ev = new MouseEvent("dragover", { bubbles: true, cancelable: true, clientX });
      Object.defineProperty(ev, "dataTransfer", { value: dt() });
      fireEvent(el, ev);
    };
    fireEvent.dragStart(tab("Closed chat"), { dataTransfer: dt() });
    dragOverAt(tab("Fix the parser"), 0);
    expect(tab("Fix the parser")).toHaveClass("drop-before");
    fireEvent.drop(tab("Fix the parser"), { dataTransfer: dt() });
    expect(useStore.getState().chatTabs[W]).toEqual(["s3", "s1", "s2"]);

    fireEvent.dragStart(tab("Closed chat"), { dataTransfer: dt() });
    dragOverAt(tab("Fix the parser"), 10);
    expect(tab("Fix the parser")).toHaveClass("drop-after");
    fireEvent.drop(tab("Fix the parser"), { dataTransfer: dt() });
    expect(useStore.getState().chatTabs[W]).toEqual(["s1", "s3", "s2"]);
    // A file drag is none of the strip's business.
    fireEvent.drop(tab("Fix the parser"), { dataTransfer: { types: ["Files"], getData: () => "" } });
    expect(useStore.getState().chatTabs[W]).toEqual(["s1", "s3", "s2"]);
  });

  it("arrow keys walk focus along the strip, wrapping; Home and End jump", () => {
    useStore.setState({ chatTabs: { [W]: ["s1", "s2", "s3"] } });
    render(<ChatTabs />);
    tab("Fix the parser").focus();
    fireEvent.keyDown(tab("Fix the parser"), { key: "ArrowRight" });
    expect(tab("Refactor tokens")).toHaveFocus();
    fireEvent.keyDown(tab("Refactor tokens"), { key: "ArrowLeft" });
    fireEvent.keyDown(tab("Fix the parser"), { key: "ArrowLeft" });
    expect(tab("Closed chat")).toHaveFocus();
    fireEvent.keyDown(tab("Closed chat"), { key: "Home" });
    expect(tab("Fix the parser")).toHaveFocus();
    fireEvent.keyDown(tab("Fix the parser"), { key: "End" });
    expect(tab("Closed chat")).toHaveFocus();
  });

  it("a chat that won't open leaves a notice and keeps its tab", async () => {
    ipc.resumeSession.mockRejectedValueOnce(new Error("no such session"));
    render(<ChatTabs />);
    await userEvent.click(tab("Refactor tokens"));
    await waitFor(() =>
      expect(useStore.getState().threads.s1?.some((i) => i.kind === "notice" && /no such session/.test(i.text))).toBe(true),
    );
    expect(useStore.getState().chatTabs[W]).toEqual(["s1", "s2"]);
  });

  it("clicking a tab opens that chat", async () => {
    render(<ChatTabs />);
    await userEvent.click(tab("Refactor tokens"));
    expect(ipc.resumeSession).toHaveBeenCalledWith("s2");
  });

  it("the × closes a tab; closing the visible chat lands on its neighbour", async () => {
    render(<ChatTabs />);
    await userEvent.click(screen.getByRole("button", { name: "Close tab: Fix the parser" }));
    expect(useStore.getState().chatTabs[W]).toEqual(["s2"]);
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s2"));
  });

  it("middle-click closes a tab too", () => {
    render(<ChatTabs />);
    fireEvent(tab("Refactor tokens"), new MouseEvent("auxclick", { button: 1, bubbles: true }));
    expect(useStore.getState().chatTabs[W]).toEqual(["s1"]);
    // A background tab: the visible chat is untouched.
    expect(ipc.resumeSession).not.toHaveBeenCalled();
  });

  it("+ starts a new chat — but not on top of an untouched one", async () => {
    render(<ChatTabs />);
    await userEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(ipc.newSession).toHaveBeenCalledOnce();
    // Now on the fresh chat: pressing + again stays put rather than minting
    // another empty session.
    await userEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(ipc.newSession).toHaveBeenCalledOnce();
  });

  it("each tab wears its chat's standing: running, needs you, something to check", () => {
    useStore.setState({
      chatTabs: { [W]: ["s1", "s2", "s3"] },
      runStatus: { s1: "running", s2: "unread" },
      approvals: { s3: { session: "s3" } as never },
    });
    render(<ChatTabs />);
    expect(tab("Fix the parser").querySelector(".status-dot.running")).not.toBeNull();
    expect(tab("Refactor tokens").querySelector(".status-dot.check")).not.toBeNull();
    expect(tab("Closed chat")).toHaveClass("needs");
    expect(tab("Closed chat").title).toMatch(/waiting on your approval/);
  });

  it("the history button counts closed chats that need you, and opens the history", async () => {
    useStore.setState({ runStatus: { s3: "unread" } });
    render(<ChatTabs />);
    expect(screen.getByLabelText("1 closed chats need you")).toHaveTextContent("1");
    await userEvent.click(screen.getByRole("button", { name: "All chats" }));
    expect(useStore.getState().historyOpen).toBe(true);
  });

  it("no badge while every needy chat has a tab", () => {
    useStore.setState({ runStatus: { s2: "unread" } });
    render(<ChatTabs />);
    expect(screen.queryByLabelText(/closed chats need you/)).toBeNull();
  });

  it("the right-click menu closes the other tabs", async () => {
    render(<ChatTabs />);
    fireEvent.contextMenu(tab("Refactor tokens"));
    await userEvent.click(screen.getByRole("option", { name: "Close others" }));
    expect(useStore.getState().chatTabs[W]).toEqual(["s2"]);
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s2"));
  });

  it("deleting from the menu asks first", async () => {
    render(<ChatTabs />);
    fireEvent.contextMenu(tab("Refactor tokens"));
    await userEvent.click(screen.getByRole("option", { name: /Delete chat/ }));
    expect(ipc.deleteSession).not.toHaveBeenCalled();
    expect(screen.getByText("Delete chat?")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^delete$/i }));
    expect(ipc.deleteSession).toHaveBeenCalledWith("s2");
  });
});

function Shortcuts() {
  useChatTabShortcuts();
  return null;
}
const press = (key: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(window, { key, ...init });

describe("chat tab shortcuts", () => {
  it("⌘W closes the visible tab, ⌘T opens a chat, ⌘K toggles the history", async () => {
    render(<Shortcuts />);
    press("w", { metaKey: true });
    expect(useStore.getState().chatTabs[W]).toEqual(["s2"]);
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s2"));

    press("t", { metaKey: true });
    await waitFor(() => expect(ipc.newSession).toHaveBeenCalledOnce());

    press("k", { metaKey: true });
    expect(useStore.getState().historyOpen).toBe(true);
    press("k", { metaKey: true });
    expect(useStore.getState().historyOpen).toBe(false);
  });

  it("⌘digit jumps to a tab, ⌃Tab cycles (wrapping), ⌘9 is the last", async () => {
    useStore.setState({ chatTabs: { [W]: ["s1", "s2", "s3"] } });
    render(<Shortcuts />);
    press("2", { metaKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s2"));
    press("Tab", { ctrlKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s3"));
    press("Tab", { ctrlKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s1"));
    press("Tab", { ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s3"));
    press("1", { metaKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s1"));
    press("9", { metaKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s3"));
  });

  it("⌘⇧→ / ⌘⇧← cycle the tabs too, wrapping at both ends", async () => {
    useStore.setState({ chatTabs: { [W]: ["s1", "s2", "s3"] } });
    render(<Shortcuts />);
    press("ArrowRight", { metaKey: true, shiftKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s2"));
    press("ArrowRight", { metaKey: true, shiftKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s3"));
    press("ArrowRight", { metaKey: true, shiftKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s1"));
    press("ArrowLeft", { metaKey: true, shiftKey: true });
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s3"));
    // Plain ⌘← (no shift) is the composer's line-start jump, not a tab switch.
    press("ArrowLeft", { metaKey: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(useStore.getState().session?.session_id).toBe("s3");
  });

  it("stands down while Home covers the chat — except the history", () => {
    useStore.setState({ homeOpen: true });
    render(<Shortcuts />);
    press("w", { metaKey: true });
    expect(useStore.getState().chatTabs[W]).toEqual(["s1", "s2"]);
    press("k", { metaKey: true });
    expect(useStore.getState().historyOpen).toBe(true);
  });

  it("stands down while the history is open, so ⌘W there never closes a tab behind it", () => {
    useStore.setState({ historyOpen: true });
    render(<Shortcuts />);
    press("w", { metaKey: true });
    expect(useStore.getState().chatTabs[W]).toEqual(["s1", "s2"]);
    press("k", { metaKey: true });
    expect(useStore.getState().historyOpen).toBe(false);
  });
});
