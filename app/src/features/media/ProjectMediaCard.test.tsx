import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ProjectMediaCard } from "./ProjectMediaCard";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaItem } from "../../lib/types";

export const item = (over: Partial<MediaItem> = {}): MediaItem => ({
  id: "g1",
  session: "s1",
  batch: "b1",
  index: 1,
  kind: "image",
  model: "flux-2-klein-4b",
  prompt: "an ox at dawn",
  params: { aspect_ratio: "16:9" },
  refs: [],
  path: "generations/2026-09-13/1402-an-ox-at-dawn-1.png",
  poster: null,
  bytes: 1000,
  width: 1024,
  height: 576,
  duration_secs: null,
  cost_usd: 0.01,
  status: "succeeded",
  error: null,
  created_at: Math.floor(Date.now() / 1000) - 30,
  completed_at: null,
  parent: null,
  seed: null,
  sources: [],
  agent_prompt: null,
  provider: null,
  ...over,
});

beforeEach(resetAll);

describe("ProjectMediaCard", () => {
  it("cold-loads the project's library and shows a quiet empty state", async () => {
    render(<ProjectMediaCard path="/w" />);
    expect(ipc.listMedia).toHaveBeenCalledWith("/w");
    expect(await screen.findByText(/No generations yet/)).toBeInTheDocument();
  });

  it("shows the newest tiles with a count and opens the gallery on the clicked one", async () => {
    ipc.listMedia.mockResolvedValue([
      item(),
      item({ id: "v1", kind: "video", prompt: "a race", duration_secs: 5, path: "generations/x.mp4" }),
      item({ id: "f1", status: "failed", path: null }),
    ]);
    const enterProject = vi.fn(async () => {});
    useStore.setState({ enterProject });
    render(<ProjectMediaCard path="/w/" />);
    expect(ipc.listMedia).toHaveBeenCalledWith("/w");
    expect(await screen.findByText("2 generations · 1 video")).toBeInTheDocument();
    // Failed items stay out of the strip.
    expect(screen.queryByText("failed")).not.toBeInTheDocument();

    await userEvent.click(await screen.findByTitle("a race"));
    expect(enterProject).toHaveBeenCalledWith("/w/");
    expect(useStore.getState().mediaFocus).toBe("v1");

    // A live event for the canonical root (no trailing slash) reaches the card.
    act(() =>
      useStore.getState().ingestMediaChanged({ session: "s1", root: "/w", items: [item({ id: "g9", prompt: "fresh" })] }),
    );
    expect(await screen.findByTitle("fresh")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Open gallery/ }));
    expect(enterProject).toHaveBeenCalledTimes(2);
  });
});
