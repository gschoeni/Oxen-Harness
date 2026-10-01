import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { MediaPanel } from "./MediaPanel";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaItem, MediaUpload } from "../../lib/types";
import type { Item } from "./thread";

const upload = (over: Partial<MediaUpload> = {}): MediaUpload => ({
  id: "u1",
  session: "s1",
  label: "[Image #1]",
  filename: "photo.png",
  kind: "image",
  bytes_sent: 640 * 1024,
  bytes_total: 1000 * 1024,
  status: "uploading",
  error: null,
  started_at: 1_789_000_000,
  ...over,
});

beforeEach(() => {
  resetAll();
  useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" } });
});

describe("MediaPanel uploads", () => {
  it("shows an upload row with its percent and hides finished ones", () => {
    useStore.setState({
      mediaUploads: {
        "/w": [upload(), upload({ id: "u2", filename: "done.png", status: "done", bytes_sent: 1, bytes_total: 1 })],
      },
    });
    render(<MediaPanel />);
    expect(screen.getByText("Uploading 1 reference…")).toBeInTheDocument();
    expect(screen.getByText("photo.png")).toBeInTheDocument();
    expect(screen.getByText("[Image #1]")).toBeInTheDocument();
    expect(screen.getByText("64% · 640 KB of 1000 KB")).toBeInTheDocument();
    expect(screen.queryByText("done.png")).not.toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Uploading photo.png" })).toHaveAttribute("aria-valuenow", "64");
  });

  it("keeps a failed upload visible with its error and stays hidden with nothing in flight", () => {
    useStore.setState({ mediaUploads: { "/w": [upload({ status: "failed", error: "hub said no" })] } });
    render(<MediaPanel />);
    expect(screen.getByText("A reference upload failed")).toBeInTheDocument();
    expect(screen.getByText("hub said no")).toBeInTheDocument();
  });

  it("renders nothing when there are no uploads or generations", () => {
    const { container } = render(<MediaPanel />);
    expect(container).toBeEmptyDOMElement();
  });
});

const generation = (over: Partial<MediaItem> = {}): MediaItem => ({
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
  path: null,
  poster: null,
  bytes: 0,
  width: null,
  height: null,
  duration_secs: null,
  cost_usd: null,
  status: "processing",
  error: null,
  created_at: Math.floor(Date.now() / 1000) - 3,
  completed_at: null,
  parent: null,
  seed: null,
  sources: [],
  agent_prompt: null,
  provider: null,
  ...over,
});

const card = (over: Partial<Extract<Item, { kind: "tool" }>> = {}): Item => ({
  id: "t1",
  kind: "tool",
  name: "generate_image",
  callId: "call_1",
  args: "{}",
  result: "",
  running: true,
  startedAt: Date.now(),
  ...over,
});

describe("MediaPanel and the in-chat generation card", () => {
  it("leaves out a generation (and its uploads) whose card is still running in the thread", () => {
    useStore.setState({
      media: { "/w": [generation()] },
      mediaUploads: { "/w": [upload()] },
      threads: { s1: [{ id: "u", kind: "user", text: "edit it" }, card()] },
    });
    const { container } = render(<MediaPanel />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows a generation that outlived its call, with its cancel", async () => {
    // A background video: the tool returned "Queued…" and the agent moved on.
    useStore.setState({
      media: { "/w": [generation({ id: "v1", kind: "video", call_id: "call_0", prompt: "a balloon rises" })] },
      threads: {
        s1: [
          { id: "u", kind: "user", text: "animate it" },
          card({ id: "t0", name: "generate_video", callId: "call_0", running: false, result: "Queued 1 video" }),
          // A later image call is on screen as its own card; the video is not.
          card(),
        ],
      },
    });
    render(<MediaPanel />);
    expect(screen.getByText("1 generation rendering")).toBeInTheDocument();
    expect(screen.getByText("a balloon rises")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Cancel video 1" }));
    expect(ipc.cancelMedia).toHaveBeenCalledWith("v1");
  });

  it("still shows a rendering generation when no card in the thread accounts for it", () => {
    // A resumed chat, or a row recorded without a call id.
    useStore.setState({ media: { "/w": [generation({ call_id: null })] }, threads: { s1: [card()] } });
    render(<MediaPanel />);
    expect(screen.getByText("1 generation rendering")).toBeInTheDocument();
  });
});
