import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { GalleryPanel } from "./GalleryPanel";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaItem } from "../../lib/types";

const item = (over: Partial<MediaItem> = {}): MediaItem => ({
  id: "g1",
  session: "s1",
  batch: "b1",
  index: 1,
  kind: "image",
  model: "flux-2-klein-4b",
  prompt: "an ox at dawn",
  params: { aspect_ratio: "16:9", seed: 7 },
  refs: ["generations/refs/abc.png"],
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
  seed: 7,
  ...over,
});

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
  });
});

describe("GalleryPanel", () => {
  it("cold-loads the library, follows change events, and cancels in-flight jobs", async () => {
    render(<GalleryPanel />);
    expect(ipc.listMedia).toHaveBeenCalledWith("/w");
    expect(await screen.findByText(/Nothing here yet/)).toBeInTheDocument();

    act(() =>
      useStore.getState().ingestMediaChanged({
        session: "s1",
        root: "/w",
        items: [
          item({ id: "v1", kind: "video", status: "processing", path: null, prompt: "a race" }),
          item(),
          item({ id: "g0", session: "other", prompt: "someone else's" }),
        ],
      }),
    );
    expect(screen.getByText("3 generations")).toBeInTheDocument();
    expect(screen.getByText(/1 rendering/)).toBeInTheDocument();
    const imgs = await screen.findAllByRole("img");
    expect(imgs).toHaveLength(2);
    expect(imgs[0].getAttribute("src")).toContain("asset://localhost//w/generations/2026-09-13/1402-an-ox-at-dawn-1.png");

    await userEvent.click(screen.getByRole("button", { name: "Cancel video generation" }));
    expect(ipc.cancelMedia).toHaveBeenCalledWith("v1");

    // Filters: only this chat's, only videos.
    await userEvent.click(screen.getByRole("button", { name: "This chat" }));
    expect(screen.getByText("2 generations")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Videos" }));
    expect(screen.getByText("1 generation")).toBeInTheDocument();
  });

  it("opens a detail pane with the prompt and stages the file as a reference", async () => {
    useStore.setState({ media: { "/w": [item()] } });
    render(<GalleryPanel />);
    const tile = await screen.findByTitle("an ox at dawn");
    await userEvent.click(tile);
    expect(screen.getByLabelText("Generation details")).toBeInTheDocument();
    expect(screen.getByText("flux-2-klein-4b")).toBeInTheDocument();
    expect(screen.getByText("$0.01")).toBeInTheDocument();
    expect(screen.getByText("16:9")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Use as reference/ }));
    expect(useStore.getState().pendingAttachments).toEqual([
      "/w/generations/2026-09-13/1402-an-ox-at-dawn-1.png",
    ]);

    await userEvent.click(screen.getByRole("button", { name: /Open in editor/ }));
    expect(useStore.getState().rightTab.s1).toBe("editor");
  });

  it("honors a focus request from a chat card", async () => {
    useStore.setState({ media: { "/w": [item(), item({ id: "g2", prompt: "second" })] }, mediaFocus: "g2" });
    render(<GalleryPanel />);
    expect(await screen.findByLabelText("Generation details")).toBeInTheDocument();
    expect(screen.getByText("second", { selector: ".gallery-detail-prompt" })).toBeInTheDocument();
    expect(useStore.getState().mediaFocus).toBeNull();
  });
});

describe("GalleryPanel layout", () => {
  it("renders the tiles inside the scroll container's explicit-row grid", () => {
    const { container } = render(<GalleryPanel />);
    act(() =>
      useStore.getState().ingestMediaChanged({
        session: "s1",
        root: "/w",
        items: [item(), item({ id: "g2", path: "generations/2026-09-13/1402-an-ox-at-dawn-2.png" })],
      }),
    );
    const scroll = container.querySelector(".gallery-scroll");
    expect(scroll).toBeTruthy();
    const grid = scroll!.querySelector(":scope > .gallery-grid");
    expect(grid).toBeTruthy();
    expect(grid!.querySelectorAll(":scope > .gallery-tile")).toHaveLength(2);
  });
});
