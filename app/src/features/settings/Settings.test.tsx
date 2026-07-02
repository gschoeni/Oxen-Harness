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
});
