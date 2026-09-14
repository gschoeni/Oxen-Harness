import { describe, expect, it } from "vitest";
import { aspectFor, generatedFiles, generationArgs } from "./media";
import { isAudioPath, isMediaPath } from "./attachments";

describe("media helpers", () => {
  it("parses saved files (and posters) out of a generation result", () => {
    const result = [
      "Generated 2 images with flux-2-klein-4b in 14s, est. $0.02:",
      "- generations/2026-09-13/1402-ox-1.png (1024×576)",
      "- generations/2026-09-13/1402-ox-2.png (1024×576)",
      "Note: model has no `seed` parameter; it was ignored",
      "- generations/2026-09-13/1417-race-1.mp4 (8.1 MB), 8s — poster generations/2026-09-13/1417-race-1.jpg",
      "- #3 failed: provider error",
      "The user sees them in the chat.",
    ].join("\n");
    expect(generatedFiles(result)).toEqual([
      { path: "generations/2026-09-13/1402-ox-1.png", kind: "image", size: "1024×576", poster: undefined },
      { path: "generations/2026-09-13/1402-ox-2.png", kind: "image", size: "1024×576", poster: undefined },
      {
        path: "generations/2026-09-13/1417-race-1.mp4",
        kind: "video",
        size: undefined,
        poster: "generations/2026-09-13/1417-race-1.jpg",
      },
    ]);
  });

  it("reads a generation call's arguments with defaults", () => {
    const g = generationArgs("generate_video", {
      prompt: "a balloon rises",
      duration: 6,
      resolution: "720p",
      refs: ["[Image #1]", 7],
      count: 9,
    });
    expect(g.kind).toBe("video");
    expect(g.duration).toBe(6);
    expect(g.refs).toEqual(["[Image #1]"]);
    expect(g.count).toBe(4);
    expect(generationArgs("generate_image", { prompt: "x" }).count).toBe(1);
  });

  it("derives a CSS aspect ratio from dimensions or the requested ratio", () => {
    expect(aspectFor({ width: 1024, height: 576 })).toBe("1024 / 576");
    expect(aspectFor(null, "9:16")).toBe("9 / 16");
    expect(aspectFor(null)).toBe("1 / 1");
  });

  it("classifies audio and media paths", () => {
    expect(isAudioPath("/tmp/track.mp3")).toBe(true);
    expect(isAudioPath("voice.WAV")).toBe(true);
    expect(isAudioPath("clip.mp4")).toBe(false);
    expect(isMediaPath("clip.mp4")).toBe(true);
    expect(isMediaPath("notes.txt")).toBe(false);
  });
});
