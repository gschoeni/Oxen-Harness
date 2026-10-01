import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { GalleryPanel } from "./GalleryPanel";
import { compareOptions } from "./CompareView";
import { lineageInputs } from "./GenerationDetail";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaItem, MediaSource } from "../../lib/types";

const source = (n: number, over: Partial<MediaSource> = {}): MediaSource => ({
  path: `generations/refs/ref${n}.png`,
  origin: "attachment",
  label: `[Image #${n}]`,
  source: `/Users/me/photo${n}.png`,
  generation: null,
  kind: "image",
  sha256: `sha${n}`,
  ...over,
});

const item = (over: Partial<MediaItem> = {}): MediaItem => ({
  id: "g1",
  session: "s1",
  turn_seq: 4,
  call_id: "call_1",
  batch: "b1",
  index: 1,
  kind: "image",
  model: "ideogram-v4-5-edit",
  prompt: "make the lamp glow",
  params: {},
  refs: [],
  path: "generations/2026-10-01/0453-lantern-harbor-1.png",
  poster: null,
  bytes: 1000,
  width: 1600,
  height: 900,
  duration_secs: null,
  cost_usd: 0.01,
  status: "succeeded",
  error: null,
  created_at: 100,
  completed_at: null,
  parent: null,
  seed: null,
  sources: [],
  agent_prompt: null,
  provider: null,
  ...over,
});

const titles = (i: MediaItem) => compareOptions(i, lineageInputs(i)).map((o) => `${o.detail}: ${o.title}`);

describe("compareOptions", () => {
  it("lists each visual input in request order, then the output", () => {
    const edited = item({
      sources: [
        source(1),
        source(2, { origin: "generation", source: "generations/2026-10-01/0425-base-1.png", generation: "g0" }),
        source(3, { path: "generations/refs/voice.mp3", source: "/Users/me/voice.mp3", kind: "audio", label: null }),
        source(4, { path: "generations/refs/clip.mov", source: "/Users/me/clip.mov", kind: "video", label: "[Video #1]" }),
      ],
      // Already one of the references: not listed twice.
      parent: "generations/2026-10-01/0425-base-1.png",
    });
    expect(titles(edited)).toEqual([
      "reference: [Image #1] photo1.png",
      "earlier generation: [Image #2] 0425-base-1.png",
      "reference: [Video #1] clip.mov",
      "output: 0453-lantern-harbor-1.png",
    ]);
    expect(compareOptions(edited, lineageInputs(edited)).map((o) => o.kind)).toEqual(["image", "image", "video", "image"]);
  });

  it("includes a parent no reference covers, and rows from before provenance", () => {
    expect(titles(item({ parent: "generations/2026-10-01/0425-base-1.png" }))).toEqual([
      "earlier file it varies: 0425-base-1.png",
      "output: 0453-lantern-harbor-1.png",
    ]);
    expect(titles(item({ refs: ["generations/refs/old.webp"] }))).toEqual([
      "reference: old.webp",
      "output: 0453-lantern-harbor-1.png",
    ]);
  });

  it("has nothing to compare without an input or without an output", () => {
    expect(titles(item())).toEqual([]);
    expect(titles(item({ path: null, status: "failed", sources: [source(1)] }))).toEqual([]);
    expect(titles(item({ sources: [source(3, { path: "generations/refs/voice.mp3" })] }))).toEqual([]);
  });
});

describe("comparing in the gallery detail", () => {
  beforeEach(() => {
    resetAll();
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" } });
  });

  async function openDetail(items: MediaItem[]) {
    useStore.setState({ media: { "/w": items } });
    const view = render(<GalleryPanel />);
    await userEvent.click(await screen.findByTitle(items[0].prompt));
    return view;
  }
  const side = (name: "A" | "B") => screen.getByRole("radio", { name: new RegExp(`^Side ${name}:`) });

  it("puts the actions right under the picture, and offers Compare only with an input", async () => {
    const { container } = await openDetail([item({ prompt: "from the prompt alone" })]);
    const actions = container.querySelector(".gallery-actions")!;
    expect(actions.previousElementSibling).toHaveClass("gallery-detail-preview");
    expect(actions.nextElementSibling).toHaveClass("gallery-detail-body");
    expect(within(actions as HTMLElement).getByRole("button", { name: /Download/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Compare with input/ })).toBeNull();
  });

  it("opens on the first reference against the output and slides between them", async () => {
    const { container } = await openDetail([item({ sources: [source(1)] })]);
    await userEvent.click(screen.getByRole("button", { name: /Compare with input/ }));
    expect(container.querySelector(".gallery-detail-preview")).toBeNull();
    expect(side("A")).toHaveAccessibleName("Side A: [Image #1] photo1.png");
    expect(side("B")).toHaveAccessibleName("Side B: 0453-lantern-harbor-1.png");
    expect(await screen.findByAltText("Side A: [Image #1] photo1.png")).toHaveAttribute(
      "src",
      expect.stringContaining("/w/generations/refs/ref1.png"),
    );
    // One input, one output: nothing to pick between.
    expect(screen.queryByRole("group", { name: /Choices for side/ })).toBeNull();

    // Side B is drawn over A and clipped at the divider.
    const divider = screen.getByRole("slider", { name: "Comparison divider" });
    expect(divider).toHaveAttribute("aria-valuenow", "50");
    const b = await screen.findByAltText("Side B: 0453-lantern-harbor-1.png");
    expect(b).toHaveStyle({ clipPath: "inset(0 0 0 50%)" });
    divider.focus();
    await userEvent.keyboard("{ArrowRight}{ArrowRight}");
    expect(divider).toHaveAttribute("aria-valuenow", "54");
    expect(b).toHaveStyle({ clipPath: "inset(0 0 0 54%)" });
    await userEvent.keyboard("{Home}");
    expect(divider).toHaveAttribute("aria-valuenow", "0");

    await userEvent.click(screen.getByRole("button", { name: "Swap sides" }));
    expect(side("A")).toHaveAccessibleName("Side A: 0453-lantern-harbor-1.png");
    await userEvent.click(screen.getByRole("button", { name: /Stop comparing/ }));
    expect(container.querySelector(".gallery-detail-preview")).not.toBeNull();
  });

  it("picks a side, then the picture that goes on it", async () => {
    await openDetail([item({ sources: [source(1), source(2), source(3)] })]);
    await userEvent.click(screen.getByRole("button", { name: /Compare with input/ }));
    // Side A is the one being changed to start with: one click swaps the reference.
    expect(side("A")).toBeChecked();
    expect(screen.getByText(/Pick what goes on side/)).toHaveTextContent("Pick what goes on side A");
    const choices = () => screen.getByRole("group", { name: /Choices for side/ });
    const choice = (name: RegExp) => within(choices()).getByRole("button", { name });
    expect(within(choices()).getAllByRole("button")).toHaveLength(4);
    expect(choice(/photo1\.png/)).toHaveAccessibleName("[Image #1] photo1.png (reference), on side A");

    await userEvent.click(choice(/photo2\.png/));
    expect(side("A")).toHaveAccessibleName("Side A: [Image #2] photo2.png");
    expect(side("B")).toHaveAccessibleName("Side B: 0453-lantern-harbor-1.png");

    // Two references against each other: switch to side B and pick one.
    await userEvent.click(side("B"));
    expect(screen.getByText(/Pick what goes on side/)).toHaveTextContent("Pick what goes on side B");
    await userEvent.click(choice(/photo3\.png/));
    expect(side("B")).toHaveAccessibleName("Side B: [Image #3] photo3.png");

    // Picking what the other side holds trades places rather than doubling up.
    await userEvent.click(choice(/photo2\.png/));
    expect(side("B")).toHaveAccessibleName("Side B: [Image #2] photo2.png");
    expect(side("A")).toHaveAccessibleName("Side A: [Image #3] photo3.png");
  });

  it("keeps comparing while stepping, and the divider's arrows don't step the gallery", async () => {
    await openDetail([
      item({ id: "g2", prompt: "second edit", created_at: 200, sources: [source(1)] }),
      item({ id: "g1", prompt: "first edit", created_at: 100, sources: [source(2)] }),
    ]);
    await userEvent.click(screen.getByRole("button", { name: /Compare with input/ }));
    const divider = screen.getByRole("slider", { name: "Comparison divider" });
    divider.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(screen.getByText("1 of 2")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Next generation" }));
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
    expect(side("A")).toHaveAccessibleName("Side A: [Image #2] photo2.png");
    // A fresh comparison for the new generation.
    expect(screen.getByRole("slider", { name: "Comparison divider" })).toHaveAttribute("aria-valuenow", "50");
  });

  it("plays two clips together and pauses them together", async () => {
    const { container } = await openDetail([
      item({
        kind: "video",
        path: "generations/2026-10-01/0500-harbor-1.mp4",
        sources: [source(1, { path: "generations/refs/in.mov", source: "/Users/me/in.mov", kind: "video", label: "[Video #1]" })],
      }),
    ]);
    await userEvent.click(screen.getByRole("button", { name: /Compare with input/ }));
    const layer = (side: string) => container.querySelector<HTMLVideoElement>(`video.compare-layer.${side}`);
    await waitFor(() => expect(layer("a") && layer("b")).toBeTruthy());
    const a = layer("a")!;
    const b = layer("b")!;
    expect(a).toHaveAttribute("src", expect.stringContaining("/w/generations/refs/in.mov"));
    // B leads; a drifting A is pulled back onto the same moment.
    Object.defineProperty(b, "currentTime", { value: 3, writable: true });
    Object.defineProperty(a, "currentTime", { value: 1, writable: true });
    fireEvent.timeUpdate(b);
    expect(a.currentTime).toBe(3);

    // jsdom has no media playback; stand in for it on every clip.
    const pause = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(pause).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Play" })).toBeInTheDocument();
  });
});
