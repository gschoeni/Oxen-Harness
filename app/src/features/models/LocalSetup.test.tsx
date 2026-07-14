import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { LocalSetup } from "./LocalSetup";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
});

describe("LocalSetup", () => {
  it("shows the detected machine and runtime status", async () => {
    render(<LocalSetup />);
    expect(await screen.findByText("Apple M2")).toBeInTheDocument();
    expect(screen.getByText(/GB memory/)).toBeInTheDocument();
    expect(screen.getByText(/Runtime ready/)).toBeInTheDocument();
    // Disk usage bar shows free-of-total.
    expect(await screen.findByText(/free of .*GB/)).toBeInTheDocument();
  });

  it("offers to set up the runtime when none is present", async () => {
    ipc.runtimeStatus.mockResolvedValue({
      binary: null,
      source: "none",
      managed_version: "b10002",
      can_manage: true,
    });
    render(<LocalSetup />);
    expect(await screen.findByRole("button", { name: /set up runtime/i })).toBeInTheDocument();
  });

  it("lists installed models and starts one on Use", async () => {
    render(<LocalSetup />);
    expect(await screen.findByText("Qwen3 8B · Q4_K_M")).toBeInTheDocument();
    await userEvent.click(screen.getAllByRole("button", { name: /^use$/i })[0]);
    await waitFor(() => expect(ipc.useLocalModel).toHaveBeenCalledWith("qwen3-8b-q4-k-m"));
  });

  it("autocompletes Hugging Face and loads a suggestion into quants to download", async () => {
    ipc.searchHfModels.mockResolvedValue([
      { repo: "bartowski/Qwen_Qwen3-8B-GGUF", downloads: 12000, likes: 30, params: "8B" },
    ]);
    ipc.resolveHfModel.mockResolvedValue({
      id: "my-model",
      display: "owner/my-model",
      params: "7B",
      context: 0,
      note: "",
      source: "huggingface",
      recommended_quant: "Q4_K_M",
      best_fit: "good",
      quants: [
        {
          quant: "Q4_K_M",
          size_bytes: 4_500_000_000,
          fit: "good",
          installed: false,
          model: {
            id: "my-model",
            display: "owner/my-model · Q4_K_M",
            params: "7B",
            quant: "Q4_K_M",
            context: 0,
            size_bytes: 4_500_000_000,
            origin: { kind: "huggingface", repo: "owner/my-model", file: "m.gguf", revision: "main" },
          },
        },
      ],
    });

    render(<LocalSetup />);
    await userEvent.click(await screen.findByRole("button", { name: /hugging face/i }));
    const input = screen.getByPlaceholderText(/search hugging face/i);
    await userEvent.type(input, "qwen3");

    // Autocomplete suggestion appears (debounced search), then selecting it resolves.
    const hit = await screen.findByText("bartowski/Qwen_Qwen3-8B-GGUF");
    await waitFor(() => expect(ipc.searchHfModels).toHaveBeenCalledWith("qwen3"));
    await userEvent.click(hit);

    expect(await screen.findByText("owner/my-model")).toBeInTheDocument();
    const download = await screen.findByRole("button", { name: /download/i });
    await userEvent.click(download);
    await waitFor(() =>
      expect(ipc.downloadModel).toHaveBeenCalledWith(
        expect.objectContaining({ id: "my-model", quant: "Q4_K_M" }),
      ),
    );
  });
});
