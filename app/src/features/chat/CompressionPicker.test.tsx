import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { CompressionPicker } from "./CompressionPicker";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", compression_mode: "off" },
  });
});

describe("CompressionPicker", () => {
  it("shows the live chat's current mode on the button", () => {
    render(<CompressionPicker disabled={false} />);
    expect(screen.getByText("Compression off")).toBeInTheDocument();
  });

  it("switches the mode from the menu and reflects the refreshed session", async () => {
    render(<CompressionPicker disabled={false} />);
    await userEvent.click(screen.getByText("Compression off"));
    await userEvent.click(screen.getByText("Audit"));

    // Backend was asked to persist + apply live; the returned session info
    // (carrying the new mode) landed in the store and the button follows.
    expect(ipc.setCompressionMode).toHaveBeenCalledWith("audit");
    expect(useStore.getState().session?.compression_mode).toBe("audit");
    expect(await screen.findByText("Compression audit")).toBeInTheDocument();
  });

  it("re-picking the current mode is a no-op", async () => {
    render(<CompressionPicker disabled={false} />);
    await userEvent.click(screen.getByText("Compression off"));
    await userEvent.click(screen.getByText("Off"));
    expect(ipc.setCompressionMode).not.toHaveBeenCalled();
  });

  it("is disabled mid-turn", () => {
    render(<CompressionPicker disabled={true} />);
    expect(screen.getByText("Compression off").closest("button")).toBeDisabled();
  });
});
