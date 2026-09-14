import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ToolCall } from "./ToolCall";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { Item } from "./thread";

type ToolItem = Extract<Item, { kind: "tool" }>;

const tool = (over: Partial<ToolItem>): ToolItem => ({
  id: "t1",
  kind: "tool",
  name: "generate_image",
  args: JSON.stringify({ prompt: "an ox at dawn", count: 2, aspect_ratio: "16:9", refs: ["[Image #1]"] }),
  result: "",
  running: true,
  startedAt: Date.now(),
  ...over,
});

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
  });
});

describe("MediaToolCall", () => {
  it("shows the prompt, refs, and skeleton tiles while rendering", () => {
    const { container } = render(<ToolCall item={tool({})} />);
    expect(screen.getByText("Generating 2 images")).toBeInTheDocument();
    expect(screen.getByText("an ox at dawn")).toBeInTheDocument();
    expect(screen.getByText("[Image #1]")).toBeInTheDocument();
    expect(container.querySelectorAll(".media-tile.skeleton")).toHaveLength(2);
    expect(container.querySelector(".media-tile.skeleton")).toHaveStyle({ aspectRatio: "16 / 9" });
  });

  it("shows reference upload bars instead of the skeleton while uploads are in flight", () => {
    useStore.setState({
      mediaUploads: {
        "/w": [
          {
            id: "u1",
            session: "s1",
            label: "[Image #1]",
            filename: "photo.png",
            kind: "image",
            bytes_sent: 500,
            bytes_total: 1000,
            status: "uploading",
            error: null,
            started_at: 1,
          },
          {
            id: "u2",
            session: "s1",
            label: null,
            filename: "song.mp3",
            kind: "audio",
            bytes_sent: 10,
            bytes_total: 10,
            status: "done",
            error: null,
            started_at: 1,
          },
        ],
      },
    });
    const { container } = render(<ToolCall item={tool({})} />);
    expect(screen.getByText("Uploading references · 1 of 2 · 75%")).toBeInTheDocument();
    expect(screen.getByText("photo.png")).toBeInTheDocument();
    expect(container.querySelectorAll(".media-tile.skeleton")).toHaveLength(0);
  });

  it("renders the saved images from the result and opens the gallery on click", async () => {
    useStore.setState({
      media: {
        "/w": [
          {
            id: "g1",
            session: "s1",
            batch: "b",
            index: 1,
            kind: "image",
            model: "flux",
            prompt: "an ox at dawn",
            params: {},
            refs: [],
            path: "generations/2026-09-13/1402-an-ox-at-dawn-1.png",
            poster: null,
            bytes: 1,
            width: 1024,
            height: 576,
            duration_secs: null,
            cost_usd: 0.01,
            status: "succeeded",
            error: null,
            created_at: 1,
            completed_at: 2,
            parent: null,
            seed: null,
          },
        ],
      },
    });
    const result = [
      "Generated 2 images with flux in 14s, est. $0.02:",
      "- generations/2026-09-13/1402-an-ox-at-dawn-1.png (1024×576)",
      "- generations/2026-09-13/1402-an-ox-at-dawn-2.png (1024×576)",
    ].join("\n");
    render(<ToolCall item={tool({ running: false, result, endedAt: Date.now() })} />);
    expect(screen.getByText("Generated 2 images")).toBeInTheDocument();
    const imgs = await screen.findAllByRole("img");
    expect(imgs).toHaveLength(2);
    expect(imgs[0].getAttribute("src")).toContain("asset://localhost//w/generations/2026-09-13/1402-an-ox-at-dawn-1.png");
    await userEvent.click(imgs[0].closest("button")!);
    expect(useStore.getState().rightTab.s1).toBe("gallery");
    expect(useStore.getState().mediaFocus).toBe("g1");
  });

  it("points at Settings when the key is missing, and shows failures", async () => {
    render(
      <ToolCall
        item={tool({
          running: false,
          result: "Media generation needs your Oxen API key. Ask the user to add their key in Settings → Connection.",
        })}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: /Add it in Settings/ }));
    expect(useStore.getState().settingsOpen).toBe(true);
    expect(useStore.getState().settingsPage).toBe("connection");

    render(<ToolCall item={tool({ id: "t2", running: false, result: "tool error: invalid arguments: `aspect_ratio` must be one of 1:1" })} />);
    expect(screen.getByText("Image generation failed")).toBeInTheDocument();
    expect(screen.getByText(/must be one of 1:1/)).toBeInTheDocument();
  });
});

describe("MediaToolCall drag sources", () => {
  it("stamps a drag from a finished tile with the file's absolute path", () => {
    const { container } = render(
      <ToolCall
        item={tool({
          running: false,
          result: "Generated 1 image with flux:\n- generations/2026-09-14/0413-ox-1.png (1360×768)",
        })}
      />,
    );
    const tile = container.querySelector(".media-tile") as HTMLElement;
    expect(tile).toBeTruthy();
    expect(tile.getAttribute("draggable")).toBe("true");
    const setData = vi.fn();
    fireEvent.dragStart(tile, { dataTransfer: { setData, types: [], effectAllowed: "none" } });
    expect(setData).toHaveBeenCalledWith(
      "application/x-oxen-workspace-files",
      JSON.stringify(["/w/generations/2026-09-14/0413-ox-1.png"]),
    );
  });
});

describe("MediaToolCall: model label", () => {
  it("names the default model when the call left it out", async () => {
    const { render, screen } = await import("@testing-library/react");
    const { act } = await import("@testing-library/react");
    const { useStore } = await import("../../lib/store");
    const { ToolCall } = await import("./ToolCall");
    act(() => useStore.setState({ mediaPrefs: { ...ipc.sampleMediaPrefs, default_image_model: "flux-2-klein-4b" } }));
    render(
      <ToolCall
        item={{ id: "t1", kind: "tool", name: "generate_image", args: JSON.stringify({ prompt: "an ox" }), result: "", running: true, startedAt: Date.now() }}
      />,
    );
    expect(await screen.findByText(/flux-2-klein-4b/)).toBeInTheDocument();
  });
});
