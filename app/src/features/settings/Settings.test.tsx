import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Settings } from "./Settings";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: ipc.sampleSession,
    theme: ipc.sampleTheme,
    mode: "dark",
    settingsOpen: true,
    settingsPage: "connection",
  });
});

describe("Settings", () => {
  it("shows the active project context in the rail", () => {
    render(<Settings />);
    const chip = document.querySelector(".settings-rail-project-name");
    // No named project in the store, so the chip falls back to the workspace basename.
    expect(chip?.textContent).toBe("project");
  });

  it("shows the current session info on the Connection page", () => {
    render(<Settings />);
    expect(screen.getByText("claude-opus-4-8")).toBeInTheDocument();
    expect(screen.getByText("/Users/dev/project")).toBeInTheDocument();
    expect(screen.getByText("current-")).toBeInTheDocument(); // session id, sliced to 8
  });

  it("navigates between subpages via the rail", async () => {
    render(<Settings />);
    // Jump to Cloud models — the catalog renders the built-in model ids.
    await userEvent.click(screen.getByRole("button", { name: /cloud models/i }));
    expect(await screen.findByText("Claude Sonnet 4.6")).toBeInTheDocument();
  });

  it("toggles light/dark mode on the Appearance page", async () => {
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /appearance/i }));
    await userEvent.click(await screen.findByRole("button", { name: /dark mode/i }));
    expect(useStore.getState().mode).toBe("light");
  });

  it("closes via the header close button", async () => {
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /close settings/i }));
    expect(useStore.getState().settingsOpen).toBe(false);
  });

  it("offers the three compression modes and persists a change", async () => {
    ipc.getCompressionMode.mockResolvedValueOnce("audit");
    ipc.totalTokensSaved.mockResolvedValueOnce(12345);
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /compression/i }));

    const select = await screen.findByRole("combobox", { name: /compression mode/i });
    // The persisted mode loads into the select, and all three modes are offered.
    await vi.waitFor(() => expect(select).toHaveValue("audit"));
    expect(
      screen.getAllByRole("option").map((o) => (o as HTMLOptionElement).value),
    ).toEqual(["off", "audit", "on"]);
    // The all-time savings stat is shown.
    expect(screen.getByText("12,345")).toBeInTheDocument();

    await userEvent.selectOptions(select, "on");
    expect(ipc.setCompressionMode).toHaveBeenCalledWith("on");
  });

  it("shows the per-model usage breakdown and total on the Usage page", async () => {
    ipc.modelUsageBreakdown.mockResolvedValueOnce({
      rows: [
        { model: "claude-sonnet-5", source: "oxen_cloud", prompt_tokens: 9885, completion_tokens: 0, cost_usd: 0.0346 },
        { model: "gemini-2-5-flash", source: "oxen_cloud", prompt_tokens: 4942, completion_tokens: 0, cost_usd: 0.0015 },
      ],
      total_cost_usd: 0.0361,
      prompt_tokens: 14827,
      completion_tokens: 0,
      has_unpriced_usage: false,
    });
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /usage/i }));

    // Both models are listed with their spend, and the grand total is shown.
    expect(await screen.findByText("claude-sonnet-5")).toBeInTheDocument();
    expect(screen.getByText("gemini-2-5-flash")).toBeInTheDocument();
    expect(screen.getByText("$0.03")).toBeInTheDocument(); // claude row cost
    // The total appears (in the "dollars spent (all time)" stat and the table
    // footer), so there are at least two occurrences of the rounded figure.
    expect(screen.getAllByText("$0.04").length).toBeGreaterThanOrEqual(1);
  });

  it("loads a selected activity day's report from the contribution grid", async () => {
    const year = new Date().getFullYear();
    const date = `${year}-01-15`;
    ipc.dailyUsage.mockResolvedValueOnce([
      { date, prompt_tokens: 1200, completion_tokens: 300 },
    ]);
    ipc.modelUsageBreakdown
      .mockResolvedValueOnce({
        rows: [], total_cost_usd: 0, prompt_tokens: 0, completion_tokens: 0,
        has_unpriced_usage: false,
      })
      .mockResolvedValueOnce({
        rows: [{
          model: "claude-opus-4-8", source: "oxen_cloud", prompt_tokens: 1200,
          completion_tokens: 300, cost_usd: 0.02,
        }],
        total_cost_usd: 0.02, prompt_tokens: 1200, completion_tokens: 300,
        has_unpriced_usage: false,
      });

    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /usage/i }));
    const day = await screen.findByRole("button", { name: /1,500 tokens/i });
    await userEvent.click(day);

    expect(ipc.modelUsageBreakdown).toHaveBeenLastCalledWith(date);
    expect(await screen.findByRole("heading", { name: /january 15/i })).toBeInTheDocument();
    expect(screen.getByText("claude-opus-4-8")).toBeInTheDocument();
  });
});
