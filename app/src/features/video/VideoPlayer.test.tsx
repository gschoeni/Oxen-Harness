import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("./videoFrames", () => ({
  loadVideoFrameIndex: vi.fn(),
  extractFramePng: vi.fn(),
  captureElementFrame: vi.fn(),
}));

import { VideoPlayer } from "./VideoPlayer";
import * as frames from "./videoFrames";
import * as ipc from "../../test/ipcMock";

const SRC = "asset://localhost/w/clips/clip.mp4";

/** jsdom has no media pipeline: give the element a settable playhead, a
 *  duration, and play/pause that flip `paused` and fire the events. */
function fakeMedia(video: HTMLVideoElement, duration = 2) {
  let time = 0;
  let paused = true;
  Object.defineProperties(video, {
    duration: { configurable: true, get: () => duration },
    readyState: { configurable: true, get: () => 4 },
    paused: { configurable: true, get: () => paused },
    currentTime: {
      configurable: true,
      get: () => time,
      set: (t: number) => {
        time = t;
      },
    },
  });
  video.play = vi.fn(async () => {
    paused = false;
    video.dispatchEvent(new Event("play"));
  });
  video.pause = vi.fn(() => {
    if (paused) return;
    paused = true;
    video.dispatchEvent(new Event("pause"));
  });
}

async function renderPlayer() {
  const view = render(<VideoPlayer src={SRC} path="clips/clip.mp4" />);
  const video = document.querySelector("video") as HTMLVideoElement;
  fakeMedia(video);
  await act(async () => {
    fireEvent.loadedMetadata(video);
  });
  return { ...view, video };
}

beforeEach(() => {
  ipc.resetIpc();
  vi.mocked(frames.loadVideoFrameIndex).mockReset().mockResolvedValue(null);
  vi.mocked(frames.extractFramePng).mockReset().mockResolvedValue(null);
  vi.mocked(frames.captureElementFrame).mockReset().mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  document.querySelectorAll(".video-host").forEach((el) => el.remove());
  // jsdom has no rvfc; a test that adds it takes it away again.
  delete (HTMLVideoElement.prototype as Partial<HTMLVideoElement>).requestVideoFrameCallback;
  delete (HTMLVideoElement.prototype as Partial<HTMLVideoElement>).cancelVideoFrameCallback;
});

describe("VideoPlayer", () => {
  it("takes over the window in theater mode with the same video element", async () => {
    const user = userEvent.setup();
    const { video, container } = await renderPlayer();
    expect(container.contains(video)).toBe(true);

    await user.click(screen.getByRole("button", { name: "Theater mode" }));
    const theater = document.querySelector(".video-theater");
    expect(theater?.parentElement).toBe(document.body);
    // Moved, not remounted: the playhead and decode state come along.
    expect(theater?.contains(video)).toBe(true);
    expect(screen.getByText("clip.mp4")).toBeTruthy();

    await user.keyboard("{Escape}");
    expect(document.querySelector(".video-theater")).toBeNull();
    expect(container.contains(video)).toBe(true);
  });

  it("drives playback and theater from the keyboard once focused", async () => {
    const user = userEvent.setup();
    const { video } = await renderPlayer();
    await user.click(video.parentElement as HTMLElement);
    expect(video.play).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Pause" })).toBeTruthy();

    await user.keyboard(" ");
    expect(video.pause).toHaveBeenCalled();
    await user.keyboard("f");
    expect(document.querySelector(".video-theater")).not.toBeNull();
    await user.keyboard("f");
    expect(document.querySelector(".video-theater")).toBeNull();
  });

  it.each([
    ["confirming frames with requestVideoFrameCallback", true],
    ["resolving frames from seeks alone", false],
  ])("steps to the middle of the file's real frames, %s", async (_how, rvfc) => {
    // A 10 fps clip whose third frame runs long: the estimate would be 30 fps.
    vi.mocked(frames.loadVideoFrameIndex).mockResolvedValue({
      timestamps: [0, 0.1, 0.2, 0.5, 0.6],
      fps: 10,
    });
    let paint: VideoFrameRequestCallback | null = null;
    if (rvfc) {
      HTMLVideoElement.prototype.requestVideoFrameCallback = (cb) => {
        paint = cb;
        return 1;
      };
      HTMLVideoElement.prototype.cancelVideoFrameCallback = () => {};
    }
    const user = userEvent.setup();
    const { video } = await renderPlayer();
    await user.click(screen.getByRole("button", { name: "Next frame" }));
    expect(video.currentTime).toBeCloseTo(0.15);
    // The next seek waits for this one to land (and, with rvfc, to paint).
    await act(async () => {
      fireEvent.seeked(video);
      paint?.(0, { mediaTime: 0.1 } as VideoFrameCallbackMetadata);
    });
    expect(screen.getByRole("slider", { name: "Video timeline" }).getAttribute("aria-valuetext")).toContain(
      "frame 2/5",
    );
    await user.click(screen.getByRole("button", { name: "Next frame" }));
    expect(video.currentTime).toBeCloseTo(0.35);
  });

  it("saves the exact frame to Downloads under the video's name", async () => {
    // jsdom's Blob has no arrayBuffer(); the webview's does.
    const png = { arrayBuffer: async () => new Uint8Array([137, 80, 78, 71]).buffer } as Blob;
    vi.mocked(frames.extractFramePng).mockResolvedValue(png);
    const user = userEvent.setup();
    await renderPlayer();
    await user.click(screen.getByRole("button", { name: "Save frame as PNG" }));

    expect(frames.extractFramePng).toHaveBeenCalledWith(SRC, 0);
    expect(frames.captureElementFrame).not.toHaveBeenCalled();
    const [name, bytes] = ipc.fsSaveDownload.mock.calls[0];
    expect(name).toBe("clip_frame_1.png");
    expect(Array.from(bytes)).toEqual([137, 80, 78, 71]);
    expect((await screen.findByRole("status")).textContent).toBe(
      "Saved clip_frame_1.png to Downloads",
    );
  });

  it("falls back to the displayed pixels, and says so when even that fails", async () => {
    const user = userEvent.setup();
    const { video } = await renderPlayer();
    await user.click(screen.getByRole("button", { name: "Save frame as PNG" }));

    expect(frames.captureElementFrame).toHaveBeenCalledWith(video);
    expect(ipc.fsSaveDownload).not.toHaveBeenCalled();
    const note = await screen.findByRole("status");
    expect(note.textContent).toBe("Couldn't save frame 1");
    expect(note.getAttribute("title")).toContain("can't be read here");
  });
});
