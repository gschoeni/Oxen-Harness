import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { UpdatesPage } from "./UpdatesPage";
import { UpdatePill } from "./UpdatePill";
import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";
import { checkForUpdate, installUpdate, openExternal, sampleUpdate } from "../../test/ipcMock";
import { useUpdate } from "./update";

const newer = {
  ...sampleUpdate,
  latest: "0.0.8",
  available: true,
  notes: "## What's changed\n- Drop files into the tree",
  release_url: "https://github.com/gschoeni/Oxen-Harness/releases/tag/app-v0.0.8",
};

beforeEach(() => {
  resetAll();
});

describe("app updates", () => {
  it("says the installed version is the latest", async () => {
    render(<UpdatesPage />);
    await useUpdate.getState().check(false);
    expect(await screen.findByText("You're on the latest version")).toBeInTheDocument();
    expect(screen.getAllByText("v0.0.7")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Update & restart" })).toBeNull();
  });

  it("puts an update pill in the title bar that opens the Updates page", async () => {
    checkForUpdate.mockResolvedValue(newer);
    render(<UpdatePill />);
    await userEvent.click(await screen.findByRole("button", { name: /Update to v0\.0\.8/ }));
    expect(checkForUpdate).toHaveBeenCalledWith(false);
    expect(useStore.getState().settingsOpen).toBe(true);
    expect(useStore.getState().settingsPage).toBe("updates");
  });

  it("installs from one click, showing the release notes", async () => {
    useUpdate.setState({ status: newer });
    render(<UpdatesPage />);
    expect(screen.getByText("Version 0.0.8 is available")).toBeInTheDocument();
    expect(screen.getByText("Drop files into the tree")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Update & restart" }));
    expect(installUpdate).toHaveBeenCalledTimes(1);
  });

  it("asks before restarting over running chats, and shows a failed install", async () => {
    installUpdate.mockRejectedValueOnce("could not install the update: permission denied");
    useStore.setState({ runStatus: { a: "running", b: "unread" } });
    useUpdate.setState({ status: newer });
    render(<UpdatesPage />);
    await userEvent.click(screen.getByRole("button", { name: "Update & restart" }));
    expect(installUpdate).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("A chat is still running");

    await userEvent.click(screen.getByRole("button", { name: "Restart anyway" }));
    expect(installUpdate).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("permission denied"));
  });

  it("links to the installer where the app can't update in place", async () => {
    useUpdate.setState({ status: { ...newer, installable: false } });
    render(<UpdatesPage />);
    await userEvent.click(screen.getByRole("button", { name: /Download v0\.0\.8/ }));
    expect(openExternal).toHaveBeenCalledWith(newer.release_url);
  });

  it("forces a fresh check from Check now", async () => {
    render(<UpdatesPage />);
    await userEvent.click(screen.getByRole("button", { name: /Check now/ }));
    expect(checkForUpdate).toHaveBeenCalledWith(true);
  });
});
