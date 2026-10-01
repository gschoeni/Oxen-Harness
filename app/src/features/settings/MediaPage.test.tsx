import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { MediaPage } from "./MediaPage";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(resetAll);

describe("MediaPage", () => {
  it("loads the saved preferences and saves a changed budget", async () => {
    ipc.listMediaModels.mockImplementation(async (kind?: string) =>
      kind === "image"
        ? [
            { id: "black-forest-labs-flux-2-klein-4b", kind: "image" as const, price: "$0.01/image", developer: null, summary: null, inputs: [] },
            { id: "nano-banana-2", kind: "image" as const, price: "$0.13/image", developer: null, summary: null, inputs: [] },
          ]
        : [],
    );
    render(<MediaPage />);
    const perRun = (await screen.findByLabelText(/Per run/)) as HTMLInputElement;
    expect(perRun.value).toBe("1");

    await userEvent.clear(perRun);
    await userEvent.type(perRun, "2.5");
    await userEvent.tab();
    await waitFor(() => expect(ipc.setMediaPrefs).toHaveBeenCalled());
    expect(ipc.setMediaPrefs).toHaveBeenLastCalledWith({ ...ipc.sampleMediaPrefs, per_run_usd: 2.5 });
    expect(await screen.findByText(/Saved/)).toBeInTheDocument();

    // "Always ask" clears the limit.
    await userEvent.click(screen.getByRole("checkbox", { name: /Always ask before a per generation/ }));
    expect(ipc.setMediaPrefs).toHaveBeenLastCalledWith({
      ...ipc.sampleMediaPrefs,
      per_run_usd: 2.5,
      per_generation_usd: null,
    });

    // The image model select lists the catalog with prices; video falls back
    // to a text field because the catalog returned nothing.
    await userEvent.click(screen.getByRole("combobox", { name: "Image" }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "black-forest-labs-flux-2-klein-4b$0.01/image",
      "nano-banana-2$0.13/image",
    ]);
    // The catalog is long, so the picker filters as you type.
    await userEvent.keyboard("banana");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await userEvent.keyboard("{Enter}");
    expect(ipc.setMediaPrefs).toHaveBeenLastCalledWith(
      expect.objectContaining({ default_image_model: "nano-banana-2" }),
    );
    expect((screen.getByLabelText("Video") as HTMLInputElement).tagName).toBe("INPUT");
  });

  it("saves the hub repo and the Oxen commit switch", async () => {
    render(<MediaPage />);
    const repo = (await screen.findByLabelText("Hub repo")) as HTMLInputElement;
    expect(repo.value).toBe("");
    await userEvent.type(repo, "/ox/my-generations/");
    await userEvent.tab();
    await waitFor(() =>
      expect(ipc.setMediaPrefs).toHaveBeenLastCalledWith({ ...ipc.sampleMediaPrefs, hub_repo: "ox/my-generations" }),
    );
    await userEvent.click(screen.getByRole("checkbox", { name: /Enable commit generations with Oxen/ }));
    expect(ipc.setMediaPrefs).toHaveBeenLastCalledWith({
      ...ipc.sampleMediaPrefs,
      hub_repo: "ox/my-generations",
      commit_with_oxen: true,
    });
    expect(await screen.findByText(/Saved/)).toBeInTheDocument();
  });

  it("reverts when a save fails", async () => {
    ipc.setMediaPrefs.mockRejectedValueOnce(new Error("disk full"));
    render(<MediaPage />);
    const folder = (await screen.findByLabelText("Folder")) as HTMLInputElement;
    await userEvent.clear(folder);
    await userEvent.type(folder, "art");
    await userEvent.tab();
    expect(await screen.findByText(/disk full/)).toBeInTheDocument();
    expect(folder.value).toBe("generations");
  });
});
