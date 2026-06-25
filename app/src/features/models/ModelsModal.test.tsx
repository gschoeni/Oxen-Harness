import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ModelsModal } from "./ModelsModal";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({ modelsOpen: true });
});

describe("ModelsModal", () => {
  it("lists available models with their details", async () => {
    render(<ModelsModal />);
    expect(await screen.findByText("Qwen2.5 Coder 7B")).toBeInTheDocument();
    expect(screen.getByText("Llama 3.2 3B")).toBeInTheDocument();
  });

  it("downloads a model that isn't on disk", async () => {
    render(<ModelsModal />);
    const row = (await screen.findByText("Qwen2.5 Coder 7B")).closest(".model-row")!;
    await userEvent.click(within(row as HTMLElement).getByRole("button", { name: /download/i }));
    expect(ipc.pullModel).toHaveBeenCalledWith("qwen2.5-coder-7b");
  });

  it("switches to an installed model and closes", async () => {
    render(<ModelsModal />);
    const row = (await screen.findByText("Llama 3.2 3B")).closest(".model-row")!;
    await userEvent.click(within(row as HTMLElement).getByRole("button", { name: /^use$/i }));
    expect(ipc.useLocalModel).toHaveBeenCalledWith("llama-3.2-3b");
    await waitFor(() => expect(useStore.getState().modelsOpen).toBe(false));
    expect(useStore.getState().session?.session_id).toBe("local-session");
  });

  it("warns when llama-server is not installed", async () => {
    ipc.listModels.mockResolvedValueOnce({ ...ipc.sampleModels, llama_installed: false });
    render(<ModelsModal />);
    expect(await screen.findByText(/llama-server isn't installed/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /install llama/i })).toBeInTheDocument();
  });
});
