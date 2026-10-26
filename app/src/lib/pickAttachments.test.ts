import { describe, expect, it, vi } from "vitest";

const open = vi.fn(async (_opts: unknown) => ["/abs/clip.mp4", "/abs/track.mp3"]);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open, save: vi.fn() }));

const { pickAttachments } = await import("./ipc");

describe("pickAttachments", () => {
  it("lets the user pick every kind a chat can attach", async () => {
    await expect(pickAttachments()).resolves.toEqual(["/abs/clip.mp4", "/abs/track.mp3"]);
    const opts = open.mock.calls[0][0] as { filters: { extensions: string[] }[] };
    // The dialog applies the first filter, so it must admit them all.
    expect(opts.filters[0].extensions).toEqual(
      expect.arrayContaining(["png", "jpg", "pdf", "mp4", "mov", "mp3", "wav", "m4a"]),
    );
  });
});
