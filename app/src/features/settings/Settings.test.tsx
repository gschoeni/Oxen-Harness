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
  });
});

describe("Settings", () => {
  it("shows the current session info", () => {
    render(<Settings />);
    expect(screen.getByText("claude-opus-4-8")).toBeInTheDocument();
    expect(screen.getByText("/Users/dev/project")).toBeInTheDocument();
    expect(screen.getByText("current-")).toBeInTheDocument(); // session id, sliced to 8
    expect(screen.getByText("Oregon Trail")).toBeInTheDocument();
  });

  it("toggles light/dark mode", async () => {
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /dark/i }));
    expect(useStore.getState().mode).toBe("light");
  });

  it("opens the local models and themes modals", async () => {
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /local models/i }));
    expect(useStore.getState().modelsOpen).toBe(true);

    await userEvent.click(screen.getByRole("button", { name: /theme/i }));
    expect(useStore.getState().themesOpen).toBe(true);
  });

  it("closes via the header close button", async () => {
    useStore.setState({ settingsOpen: true });
    render(<Settings />);
    await userEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(useStore.getState().settingsOpen).toBe(false);
  });
});
