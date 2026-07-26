import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { TrailStrip } from "./TrailStrip";
import { useStore } from "../../lib/store";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";
import type { LedgerEntry } from "../../lib/types";

const NOW = Math.floor(Date.now() / 1000);

function entry(overrides: Partial<LedgerEntry> = {}): LedgerEntry {
  return {
    id: "cur",
    workspace: "/work/app",
    model: "m",
    created_at: NOW - 7_200,
    last_activity_at: NOW - 600,
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

function seedChat(e: LedgerEntry, running: string[] = []) {
  const snapshot = { entries: [e], running, last_seen: 0 };
  useStore.setState({
    ledger: snapshot,
    session: {
      model: "m",
      workspace: "/work/app",
      session_id: "cur",
      tokens_used: 0,
      context_tokens: 0,
      context_window: 200_000,
      compression_mode: "off",
    },
  });
  vi.mocked(ipc.ledgerSnapshot).mockResolvedValue(snapshot);
}

beforeEach(() => {
  resetAll();
});

describe("the chat's pinned trail", () => {
  it("pins the thread's trail with a one-click tie-off", async () => {
    seedChat(entry());
    const { container } = render(<TrailStrip />);
    expect(container.querySelector(".trail")).toBeTruthy();
    expect(screen.getByText(/done ·/)).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: /tie the knot/i }));
    await waitFor(() => expect(ipc.settleSession).toHaveBeenCalledWith("cur", ""), {
      timeout: 2_000,
    });
  });

  it("offers no tie-off while the agent is riding", () => {
    seedChat(entry(), ["cur"]);
    render(<TrailStrip />);
    expect(screen.queryByRole("button", { name: /tie the knot/i })).toBeNull();
  });

  it("a settled thread can be untied from its chat", async () => {
    seedChat(entry({ settle: { settled_at: NOW - 60, note: "" } }));
    render(<TrailStrip />);
    await userEvent.click(screen.getByRole("button", { name: "untie" }));
    await waitFor(() => expect(ipc.reopenSession).toHaveBeenCalledWith("cur"));
  });

  it("renders nothing for a session the board doesn't know", () => {
    useStore.setState({ ledger: { entries: [], running: [], last_seen: 0 } });
    const { container } = render(<TrailStrip />);
    expect(container.querySelector(".chat-trail")).toBeNull();
  });
});
