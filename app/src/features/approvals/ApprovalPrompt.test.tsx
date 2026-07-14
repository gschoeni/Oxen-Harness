import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ApprovalPrompt } from "./ApprovalPrompt";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { ApprovalRequestEvent } from "../../lib/types";

const request = (overrides: Partial<ApprovalRequestEvent> = {}): ApprovalRequestEvent => ({
  session: "s1",
  id: "a0",
  kind: "shell",
  tool: "run_shell",
  command: "rm -rf ./build",
  risk: "dangerous",
  reasons: ["deletes files (rm)"],
  grant_label: "this exact command",
  offer_project_grant: true,
  offer_trash: true,
  ...overrides,
});

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    threads: { s1: [] },
    approvals: {},
  });
});

describe("ApprovalPrompt", () => {
  it("renders nothing without a pending approval for the visible chat", () => {
    // A background chat's approval must not pop into this one.
    act(() => useStore.getState().ingestApprovalRequest(request({ session: "other" })));
    const { container } = render(<ApprovalPrompt />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the command, risk, and reasons, and answers run-once", async () => {
    act(() => useStore.getState().ingestApprovalRequest(request()));
    render(<ApprovalPrompt />);

    expect(screen.getByText("rm -rf ./build")).toBeInTheDocument();
    expect(screen.getByText("dangerous")).toBeInTheDocument();
    expect(screen.getByText(/deletes files/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Run once" }));
    expect(ipc.answerApproval).toHaveBeenCalledWith("a0", "once", undefined);
    // The card clears immediately (the resolved event only adds the notice).
    expect(useStore.getState().approvals["s1"]).toBeUndefined();
  });

  it("offers project and trash options only when the request does", () => {
    act(() =>
      useStore
        .getState()
        .ingestApprovalRequest(request({ offer_project_grant: false, offer_trash: false })),
    );
    render(<ApprovalPrompt />);
    expect(screen.queryByRole("button", { name: "Allow for project" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Move to trash instead" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Allow for session" })).toBeInTheDocument();
  });

  it("denies with the user's typed reason", async () => {
    act(() => useStore.getState().ingestApprovalRequest(request()));
    render(<ApprovalPrompt />);

    await userEvent.type(
      screen.getByPlaceholderText(/deny with a reason/i),
      "that folder is still needed",
    );
    await userEvent.click(screen.getByRole("button", { name: "Deny with reason" }));
    expect(ipc.answerApproval).toHaveBeenCalledWith("a0", "deny", "that folder is still needed");
  });

  it("clears the card and leaves a thread notice when the approval resolves", () => {
    act(() => useStore.getState().ingestApprovalRequest(request()));
    act(() =>
      useStore.getState().ingestApprovalResolved({
        session: "s1",
        phase: "resolved",
        name: "run_shell",
        command: "rm -rf ./build",
        decision: "approved",
      }),
    );
    expect(useStore.getState().approvals["s1"]).toBeUndefined();
    const thread = useStore.getState().threads["s1"];
    const notice = thread.find((i) => i.kind === "notice");
    expect(notice && "text" in notice ? notice.text : "").toContain("approved");
  });
});
