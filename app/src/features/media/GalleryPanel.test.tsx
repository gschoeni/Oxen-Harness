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
  turn_seq: 4,
  call_id: "call_1",
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
  sources: [],
  agent_prompt: null,
  provider: null,
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

  it("shows the grid or one generation, never both, and the way back keeps the tile", async () => {
    useStore.setState({ media: { "/w": [item(), item({ id: "g2", prompt: "second" })] } });
    const { container } = render(<GalleryPanel />);
    await userEvent.click(await screen.findByTitle("second"));
    expect(screen.getByLabelText("Generation details")).toBeInTheDocument();
    expect(container.querySelector(".gallery-grid")).toBeNull();
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Filter" })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Back to all generations" }));
    expect(screen.queryByLabelText("Generation details")).toBeNull();
    expect(container.querySelectorAll(".gallery-tile")).toHaveLength(2);
    expect(screen.getByTitle("second")).toHaveClass("selected");
  });

  it("steps through the filtered feed with the header buttons and the arrow keys", async () => {
    useStore.setState({
      media: {
        "/w": [
          item({ id: "g3", prompt: "third", created_at: 3 }),
          item({ id: "v1", kind: "video", prompt: "a clip", created_at: 2 }),
          item({ id: "g1", prompt: "first", created_at: 1 }),
        ],
      },
    });
    render(<GalleryPanel />);
    await userEvent.click(screen.getByRole("button", { name: "Images" }));
    await userEvent.click(await screen.findByTitle("third"));
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous generation" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: "Next generation" }));
    expect(screen.getByText("first", { selector: ".gallery-detail-prompt" })).toBeInTheDocument();
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next generation" })).toBeDisabled();

    await userEvent.keyboard("{ArrowLeft}");
    expect(screen.getByText("third", { selector: ".gallery-detail-prompt" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByLabelText("Generation details")).toBeNull();
    expect(screen.getByText("2 generations")).toBeInTheDocument();
  });

  it("lays out the metadata and switches to the raw manifest row", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    useStore.setState({
      media: {
        "/w": [
          item({
            agent_prompt: "an ox at dawn like [Image #1]",
            prompt: "an ox at dawn like @Image1",
            completed_at: Math.floor(Date.now() / 1000) - 9,
            provider: { status: "succeeded", seed: 7 },
          }),
        ],
      },
    });
    render(<GalleryPanel />);
    await userEvent.click(await screen.findByTitle("an ox at dawn like @Image1"));
    expect(screen.getByText("an ox at dawn like [Image #1]", { selector: ".gallery-detail-prompt" })).toBeInTheDocument();
    expect(screen.getByText(/Sent as: an ox at dawn like @Image1/)).toBeInTheDocument();
    expect(screen.getByText("flux-2-klein-4b")).toBeInTheDocument();
    expect(screen.getByText("$0.01")).toBeInTheDocument();
    expect(screen.getByText("1024×576 · 1000 B · PNG")).toBeInTheDocument();
    expect(screen.getByText("took 21s")).toBeInTheDocument();
    expect(screen.getByText("16:9")).toBeInTheDocument();
    expect(screen.getByText("this chat")).toBeInTheDocument();
    expect(screen.getByText("turn #4")).toBeInTheDocument();
    expect(screen.getByText("call_1")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Open chat/ })).toBeNull();

    await userEvent.click(screen.getByRole("tab", { name: "Raw" }));
    const raw = screen.getByText(/"id": "g1"/, { selector: "pre" });
    expect(raw.textContent).toContain('"agent_prompt": "an ox at dawn like [Image #1]"');
    expect(raw.textContent).toContain('"provider": {');
    expect(screen.queryByText("flux-2-klein-4b")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /Copy JSON/ }));
    expect(writeText).toHaveBeenCalledWith(JSON.stringify(useStore.getState().media["/w"]![0], null, 2));
    expect(await screen.findByText("Copied")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "Details" }));
    expect(screen.getByText("flux-2-klein-4b")).toBeInTheDocument();
  });

  it("walks the lineage in both directions and opens the originating chat", async () => {
    const photo = item({
      id: "p1",
      prompt: "a photo study",
      created_at: 1,
      path: "generations/2026-09-13/1400-a-photo-study-1.png",
    });
    const child = item({
      id: "c1",
      session: "elsewhere",
      prompt: "the study, animated",
      kind: "video",
      created_at: 2,
      path: "generations/2026-09-13/1401-animated-1.mp4",
      poster: "generations/2026-09-13/1401-animated-1.jpg",
      refs: ["generations/refs/aaaa.png", "generations/refs/bbbb.png"],
      sources: [
        {
          path: "generations/refs/aaaa.png",
          origin: "generation",
          label: "[Image #1]",
          source: "generations/2026-09-13/1400-a-photo-study-1.png",
          generation: "p1",
          kind: "image",
          sha256: "aaaa",
        },
        {
          path: "generations/refs/bbbb.png",
          origin: "attachment",
          label: "[Image #2]",
          source: "/Users/me/sketch.png",
          generation: null,
          kind: "image",
          sha256: "bbbb",
        },
      ],
    });
    useStore.setState({ media: { "/w": [child, photo] } });
    render(<GalleryPanel />);
    await userEvent.click(await screen.findByTitle("the study, animated"));

    // Inputs: the earlier generation opens; the attachment is just a fact.
    expect(screen.getByText("[Image #2] sketch.png")).toBeInTheDocument();
    expect(screen.getByText("attached to the chat · image")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /\[Image #1\] 1400-a-photo-study-1.png/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /sketch.png/ })).toBeNull();
    expect(screen.getByText("this video · flux-2-klein-4b")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /\[Image #1\] 1400-a-photo-study-1.png/ }));
    expect(screen.getByText("a photo study", { selector: ".gallery-detail-prompt" })).toBeInTheDocument();
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
    // …and from the photo, the clip made from it leads back.
    expect(screen.getByText("video made from this", { exact: false })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /1401-animated-1.mp4/ }));
    expect(screen.getByText("the study, animated", { selector: ".gallery-detail-prompt" })).toBeInTheDocument();

    // The clip came from another chat: the action resumes it (which swaps
    // the session, so it goes last).
    await userEvent.click(screen.getByRole("button", { name: /Open chat/ }));
    expect(ipc.resumeSession).toHaveBeenCalledWith("elsewhere");
  });

  it("opens a generation with the prompt and stages the file as a reference", async () => {
    useStore.setState({ media: { "/w": [item()] } });
    render(<GalleryPanel />);
    const tile = await screen.findByTitle("an ox at dawn");
    await userEvent.click(tile);
    expect(screen.getByLabelText("Generation details")).toBeInTheDocument();
    expect(screen.getByText("flux-2-klein-4b")).toBeInTheDocument();
    expect(screen.getByText("$0.01")).toBeInTheDocument();
    expect(screen.getByText("16:9")).toBeInTheDocument();
    // A row recorded before provenance was tracked still shows its stored copy.
    expect(screen.getByText("abc.png")).toBeInTheDocument();
    expect(screen.getByText("reference")).toBeInTheDocument();

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
