import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ThemesModal } from "./ThemesModal";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({ themesOpen: true });
});

describe("ThemesModal", () => {
  it("lists themes with built-in/custom tags", async () => {
    render(<ThemesModal />);
    expect(await screen.findByText("Midnight")).toBeInTheDocument();
    expect(screen.getByText("My Custom")).toBeInTheDocument();
    expect(screen.getAllByText("built-in").length).toBeGreaterThan(0);
  });

  it("activates a theme and applies its palette", async () => {
    ipc.useTheme.mockResolvedValueOnce({
      ...ipc.sampleTheme,
      meta: { ...ipc.sampleTheme.meta, name: "Midnight" },
    });
    render(<ThemesModal />);
    const row = (await screen.findByText("Midnight")).closest(".theme-row")!;
    await userEvent.click(within(row as HTMLElement).getByRole("button", { name: /^use$/i }));
    expect(ipc.useTheme).toHaveBeenCalledWith("Midnight");
    await waitFor(() => expect(useStore.getState().theme?.meta.name).toBe("Midnight"));
  });

  it("exports a theme to the clipboard", async () => {
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(<ThemesModal />);
    const row = (await screen.findByText("My Custom")).closest(".theme-row")!;
    await userEvent.click(within(row as HTMLElement).getByRole("button", { name: /export/i }));
    expect(ipc.exportTheme).toHaveBeenCalledWith("My Custom");
    await waitFor(() => expect(writeText).toHaveBeenCalled());
  });

  it("removes a custom theme but offers no remove for built-ins", async () => {
    render(<ThemesModal />);
    const custom = (await screen.findByText("My Custom")).closest(".theme-row")!;
    expect(within(custom as HTMLElement).queryByRole("button", { name: /remove/i })).toBeInTheDocument();

    const builtin = screen.getByText("Midnight").closest(".theme-row")!;
    expect(within(builtin as HTMLElement).queryByRole("button", { name: /remove/i })).toBeNull();

    await userEvent.click(within(custom as HTMLElement).getByRole("button", { name: /remove/i }));
    expect(ipc.removeTheme).toHaveBeenCalledWith("My Custom");
  });

  it("vibe-codes a new theme from the brief", async () => {
    render(<ThemesModal />);
    await userEvent.click(screen.getByText(/vibe-code a new theme/i));
    await userEvent.click(screen.getByRole("button", { name: /generate with the model/i }));
    expect(ipc.newTheme).toHaveBeenCalledWith(
      expect.stringContaining("Create a complete terminal theme"),
    );
  });
});
