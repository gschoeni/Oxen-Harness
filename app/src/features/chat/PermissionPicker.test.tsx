import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { PermissionPicker } from "./PermissionPicker";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const other = { ...ipc.sampleSession, session_id: "s2" };

beforeEach(() => {
  resetAll();
  const session = { ...ipc.sampleSession, session_id: "s1" };
  useStore.setState({ session, infos: { s1: session, s2: other } });
});

describe("PermissionPicker", () => {
  it("shows the mode the chat in view is running under", () => {
    render(<PermissionPicker />);
    expect(screen.getByText("Relaxed")).toBeInTheDocument();
  });

  it("switches this chat to bypass and leaves other chats alone", async () => {
    render(<PermissionPicker />);
    await userEvent.click(screen.getByText("Relaxed"));
    await userEvent.click(screen.getByText("Bypass"));

    expect(ipc.setChatPermissionMode).toHaveBeenCalledWith("s1", "bypass");
    const state = useStore.getState();
    expect(state.session?.permission_mode).toBe("bypass");
    expect(state.infos.s1.permission_mode).toBe("bypass");
    expect(state.infos.s2.permission_mode).toBe("relaxed");
    // The menu closed; the trigger now reads (and flags) the new mode.
    expect(screen.getByText("Bypass").closest("button")).toHaveClass("picker-danger");
  });

  it("re-picking the current mode is a no-op", async () => {
    render(<PermissionPicker />);
    await userEvent.click(screen.getByText("Relaxed"));
    await userEvent.click(screen.getAllByText("Relaxed")[1]);
    expect(ipc.setChatPermissionMode).not.toHaveBeenCalled();
  });

  it("says so when the switch fails, and keeps showing the real mode", async () => {
    ipc.setChatPermissionMode.mockRejectedValueOnce("unknown work context s1");
    render(<PermissionPicker />);
    await userEvent.click(screen.getByText("Relaxed"));
    await userEvent.click(screen.getByText("Bypass"));

    expect(await screen.findByRole("alert")).toHaveTextContent("unknown work context s1");
    expect(useStore.getState().session?.permission_mode).toBe("relaxed");
  });

  it("opens the Permissions settings page for the saved default", async () => {
    render(<PermissionPicker />);
    await userEvent.click(screen.getByText("Relaxed"));
    await userEvent.click(screen.getByText("Default mode & rules…"));
    expect(useStore.getState().settingsPage).toBe("permissions");
  });
});
