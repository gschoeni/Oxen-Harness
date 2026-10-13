import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { LocalMediaSection } from "./LocalMediaSection";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(resetAll);

describe("LocalMediaSection", () => {
  it("lists the on-device models with what they cost and do", async () => {
    render(<LocalMediaSection />);
    expect(await screen.findByText("Iris-3B Turbo")).toBeInTheDocument();
    expect(screen.getByText(/Apache-2.0 · 8\.5 GB · ~21s per image/)).toBeInTheDocument();
    expect(screen.getByText(/edits references/)).toBeInTheDocument();
    expect(screen.getByText("Installed")).toBeInTheDocument();
    expect(screen.getByText(/20\.0 GB free · engine stable-diffusion\.cpp master-959-d637cfb/)).toBeInTheDocument();
  });

  it("downloads with a live progress bar and refreshes when done", async () => {
    let finish: () => void = () => {};
    ipc.pullLocalMedia.mockImplementation(() => new Promise<void>((resolve) => (finish = resolve)));
    const onChanged = vi.fn();
    render(<LocalMediaSection onChanged={onChanged} />);
    await userEvent.click(await screen.findByRole("button", { name: "Download FLUX.2 [klein] 4B" }));
    expect(ipc.pullLocalMedia).toHaveBeenCalledWith("local/flux-2-klein-4b");
    act(() =>
      ipc.emit("localMediaProgress", {
        id: "local/flux-2-klein-4b",
        phase: "weights",
        file: "leejet/FLUX.2-klein-4B-GGUF/flux-2-klein-4b-Q4_0.gguf",
        done_bytes: 2_562_890_375,
        total_bytes: 5_125_780_750,
        error: null,
      }),
    );
    const bar = screen.getByRole("progressbar", { name: "Downloading FLUX.2 [klein] 4B" });
    expect(bar).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText(/Downloading · 50% · 2\.6 of 5\.1 GB/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Cancel downloading FLUX.2 [klein] 4B" }));
    expect(ipc.cancelLocalMediaPull).toHaveBeenCalledWith("local/flux-2-klein-4b");
    act(() => finish());
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(ipc.localMediaStatus.mock.calls.length).toBeGreaterThan(1);
  });

  it("removes an installed model and shows a failed download's reason", async () => {
    ipc.pullLocalMedia.mockRejectedValue("FLUX.2 [klein] 4B needs 5.1 GB more on disk and only 2.0 GB is free");
    render(<LocalMediaSection />);
    await userEvent.click(await screen.findByRole("button", { name: "Remove Iris-3B Turbo" }));
    expect(ipc.removeLocalMedia).toHaveBeenCalledWith("local/iris-3b-turbo");
    await userEvent.click(screen.getByRole("button", { name: "Download FLUX.2 [klein] 4B" }));
    expect(await screen.findByText(/needs 5\.1 GB more on disk/)).toBeInTheDocument();
  });

  it("says why local models are unavailable", async () => {
    ipc.localMediaStatus.mockResolvedValue({ ...ipc.sampleLocalMedia, models: [], error: "local media catalog: bad overlay" });
    render(<LocalMediaSection />);
    expect(await screen.findByText(/Local models are unavailable: local media catalog: bad overlay/)).toBeInTheDocument();
  });
});
