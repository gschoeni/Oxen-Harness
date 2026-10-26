import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { InlineVideo, VideoThumb, firstFrameSrc } from "./VideoThumb";

const SRC = "asset://localhost/w/clips/clip.mp4";

afterEach(cleanup);

/** jsdom has no media pipeline: play/pause just fire their events. */
function fakePlayback(video: HTMLVideoElement) {
  video.play = vi.fn(async () => {
    video.dispatchEvent(new Event("play"));
  });
  video.pause = vi.fn(() => {
    video.dispatchEvent(new Event("pause"));
  });
}

describe("firstFrameSrc", () => {
  it("asks for a frame just past the start, keeping a cache-buster", () => {
    expect(firstFrameSrc(SRC)).toBe(`${SRC}#t=0.1`);
    expect(firstFrameSrc(`${SRC}?v=2`)).toBe(`${SRC}?v=2#t=0.1`);
    expect(firstFrameSrc(`${SRC}#t=3`)).toBe(`${SRC}#t=3`);
  });
});

describe("VideoThumb", () => {
  it("shows the first frame or poster under a play badge, muted and still", () => {
    const { container } = render(<VideoThumb src={SRC} poster="asset://p.png" label="clip" />);
    const video = screen.getByLabelText("clip") as HTMLVideoElement;
    expect(video.getAttribute("src")).toBe(`${SRC}#t=0.1`);
    expect(video.getAttribute("poster")).toBe("asset://p.png");
    expect(video.muted).toBe(true);
    expect(video.hasAttribute("controls")).toBe(false);
    expect(container.querySelector(".video-still-play svg")).not.toBeNull();
  });

  it("previews on hover when asked, and the badge steps aside while it plays", () => {
    const { container } = render(<VideoThumb src={SRC} label="clip" hoverPlay />);
    const video = screen.getByLabelText("clip") as HTMLVideoElement;
    fakePlayback(video);
    const thumb = container.querySelector(".video-still") as HTMLElement;
    fireEvent.mouseEnter(thumb);
    expect(video.play).toHaveBeenCalled();
    expect(thumb.classList.contains("playing")).toBe(true);
    fireEvent.mouseLeave(thumb);
    expect(thumb.classList.contains("playing")).toBe(false);
  });

  it("doesn't play on hover by default", () => {
    const { container } = render(<VideoThumb src={SRC} label="clip" />);
    const video = screen.getByLabelText("clip") as HTMLVideoElement;
    fakePlayback(video);
    fireEvent.mouseEnter(container.querySelector(".video-still") as HTMLElement);
    expect(video.play).not.toHaveBeenCalled();
  });

  it("falls back to a film icon when the file can't load", () => {
    const { container } = render(<VideoThumb src={SRC} label="clip" />);
    fireEvent.error(screen.getByLabelText("clip"));
    expect(container.querySelector("video")).toBeNull();
    expect(screen.getByRole("img", { name: "clip" }).classList.contains("failed")).toBe(true);
  });

  it("tries again when the source changes after a failure", () => {
    const { container, rerender } = render(<VideoThumb src={SRC} label="clip" />);
    fireEvent.error(screen.getByLabelText("clip"));
    rerender(<VideoThumb src={`${SRC}?v=1`} label="clip" />);
    expect(container.querySelector("video")?.getAttribute("src")).toBe(`${SRC}?v=1#t=0.1`);
  });
});

describe("InlineVideo", () => {
  it("is a thumbnail until clicked, then the native player, playing", () => {
    const onDragStart = vi.fn();
    const { container } = render(<InlineVideo src={SRC} label="clip" onDragStart={onDragStart} />);
    const button = screen.getByRole("button", { name: "Play clip" });
    expect(button.getAttribute("draggable")).toBe("true");
    expect(container.querySelector("video[controls]")).toBeNull();

    fireEvent.click(button);
    const player = container.querySelector("video") as HTMLVideoElement;
    expect(player.hasAttribute("controls")).toBe(true);
    expect(player.autoplay).toBe(true);
    expect(player.getAttribute("src")).toBe(SRC);
    expect(player.getAttribute("draggable")).toBe("true");
    expect(screen.queryByRole("button", { name: "Play clip" })).toBeNull();
  });
});
