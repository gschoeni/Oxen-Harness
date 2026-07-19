import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { LogsPage } from "./LogsPage";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { SessionSummary } from "../../lib/types";

const native: SessionSummary = {
  id: "s1", workspace: "/w", model: "m", created_at: 1_700_000_100, title: "Native chat", message_count: 4, review_status: "", source: "",
};
const imported: SessionSummary = {
  id: "s2", workspace: "/w", model: "claude-fable-5", created_at: 1_700_000_000, title: "Imported chat", message_count: 6, review_status: "", source: "claude-code",
};

beforeEach(() => {
  resetAll();
  ipc.listSessions.mockResolvedValue([native, imported] as never);
  ipc.importSourcesScan.mockResolvedValue([
    { source: "claude-code", available: 3, imported: 1 },
    { source: "cursor", available: 0, imported: 0 },
  ]);
});

describe("LogsPage import panel", () => {
  it("lists detected sources with counts, hiding tools with nothing to offer", async () => {
    render(<LogsPage />);
    expect(
      await screen.findByText("Claude Code", { selector: ".log-import-name" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/3 conversations found/)).toBeInTheDocument();
    expect(screen.getByText(/1 imported/)).toBeInTheDocument();
    // Cursor has no conversations on this machine — no row for it.
    expect(screen.queryByText("Cursor", { selector: ".log-import-name" })).toBeNull();
    // A source with prior imports offers a rescan, not a first import.
    expect(screen.getByRole("button", { name: /Rescan/ })).toBeInTheDocument();
  });

  it("imports a source, reports what changed, and refreshes list + counts", async () => {
    ipc.importExternal.mockResolvedValue({ imported: 2, updated: 1, skipped: 1 });
    render(<LogsPage />);
    const button = await screen.findByRole("button", { name: /Rescan/ });
    const scansBefore = ipc.importSourcesScan.mock.calls.length;
    const listsBefore = ipc.listSessions.mock.calls.length;
    await userEvent.click(button);

    expect(ipc.importExternal).toHaveBeenCalledWith("claude-code");
    expect(
      await screen.findByText("Claude Code: imported 2 new, refreshed 1, 1 unchanged."),
    ).toBeInTheDocument();
    // The chat list and the source counts both re-query after an import.
    await waitFor(() =>
      expect(ipc.importSourcesScan.mock.calls.length).toBeGreaterThan(scansBefore),
    );
    expect(ipc.listSessions.mock.calls.length).toBeGreaterThan(listsBefore);
  });

  it("badges imported chats with their source and filters by source", async () => {
    render(<LogsPage />);
    expect(await screen.findByText("Imported chat")).toBeInTheDocument();
    const row = screen.getByText("Imported chat").closest(".log-trace")!;
    expect(row.querySelector(".log-source-badge")).toHaveTextContent("Claude Code");
    // Native chats carry no badge.
    const nativeRow = screen.getByText("Native chat").closest(".log-trace")!;
    expect(nativeRow.querySelector(".log-source-badge")).toBeNull();

    // "This app" narrows the list to native chats only.
    await userEvent.selectOptions(screen.getByDisplayValue("All sources"), "This app");
    expect(screen.queryByText("Imported chat")).toBeNull();
    expect(screen.getByText("Native chat")).toBeInTheDocument();
  });
});
