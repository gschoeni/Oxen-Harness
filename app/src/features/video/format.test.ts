import { describe, expect, it } from "vitest";
import { formatFrameCounter, formatTimecode, frameFilename } from "./format";

describe("video readouts", () => {
  it("formats a timecode to the millisecond", () => {
    expect(formatTimecode(0)).toBe("0:00.000");
    expect(formatTimecode(61.5)).toBe("1:01.500");
    expect(formatTimecode(-3)).toBe("0:00.000");
  });

  it("pads the frame counter to the total's width and marks estimates", () => {
    expect(formatFrameCounter(0, 720, true)).toBe("001/720");
    expect(formatFrameCounter(719, 720, true)).toBe("720/720");
    expect(formatFrameCounter(8, 30, false)).toBe("~09/30");
  });

  it("names a saved frame after the video and its 1-based frame", () => {
    expect(frameFilename("production/rust-la-trailer-30s.mp4", 11)).toBe(
      "rust-la-trailer-30s_frame_12.png",
    );
    expect(frameFilename(".mp4", 0)).toBe(".mp4_frame_1.png");
    expect(frameFilename("", 0)).toBe("video_frame_1.png");
  });
});
