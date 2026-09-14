import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { useStore } from "./store";
import * as ipc from "../test/ipcMock";
import { resetAll } from "../test/utils";
import type { MediaItem } from "./types";

export const sampleItem = (over: Partial<MediaItem> = {}): MediaItem => ({
  id: "g1",
  session: "s1",
  batch: "b1",
  index: 1,
  kind: "image",
  model: "flux",
  prompt: "an ox",
  params: { aspect_ratio: "16:9" },
  refs: [],
  path: "generations/2026-09-13/1402-an-ox-1.png",
  poster: null,
  bytes: 1000,
  width: 1024,
  height: 576,
  duration_secs: null,
  cost_usd: 0.01,
  status: "succeeded",
  error: null,
  created_at: 1_789_000_000,
  completed_at: 1_789_000_010,
  parent: null,
  seed: null,
  ...over,
});

beforeEach(resetAll);

describe("store: media library", () => {
  it("replaces a project's library on each event and cold-loads without clobbering", async () => {
    const s = useStore.getState();
    ipc.listMedia.mockResolvedValueOnce([sampleItem({ id: "old" })]);
    await s.refreshMedia("/w");
    expect(useStore.getState().media["/w"]?.map((i) => i.id)).toEqual(["old"]);

    s.ingestMediaChanged({ session: "s1", root: "/w", items: [sampleItem({ id: "g2" }), sampleItem()] });
    expect(useStore.getState().media["/w"]?.map((i) => i.id)).toEqual(["g2", "g1"]);

    // A later refresh never overwrites what an event already said.
    ipc.listMedia.mockResolvedValueOnce([sampleItem({ id: "stale" })]);
    await useStore.getState().refreshMedia("/w");
    expect(useStore.getState().media["/w"]?.map((i) => i.id)).toEqual(["g2", "g1"]);
  });

  it("opens the gallery dock with a focus and stages reference files", () => {
    useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1" } });
    useStore.getState().openGallery("g1");
    expect(useStore.getState().rightTab.s1).toBe("gallery");
    expect(useStore.getState().mediaFocus).toBe("g1");
    useStore.getState().clearMediaFocus();
    expect(useStore.getState().mediaFocus).toBeNull();

    useStore.getState().stageAttachment("/w/generations/a.png");
    useStore.getState().stageAttachment("/w/generations/a.png");
    expect(useStore.getState().pendingAttachments).toEqual(["/w/generations/a.png"]);
    expect(useStore.getState().takePendingAttachments()).toEqual(["/w/generations/a.png"]);
    expect(useStore.getState().pendingAttachments).toEqual([]);

    useStore.getState().cancelMedia("g1");
    expect(ipc.cancelMedia).toHaveBeenCalledWith("g1");
  });
});

describe("store: reference uploads", () => {
  it("keeps uploads per root and defaults to none when a payload omits them", () => {
    const s = useStore.getState();
    const upload = {
      id: "u1",
      session: "s1",
      label: "[Image #1]",
      filename: "photo.png",
      kind: "image",
      bytes_sent: 512,
      bytes_total: 1024,
      status: "uploading",
      error: null,
      started_at: 1_789_000_000,
    };
    s.ingestMediaChanged({ session: "s1", root: "/w", items: [], uploads: [upload] });
    expect(useStore.getState().mediaUploads["/w"]).toEqual([upload]);
    s.ingestMediaChanged({ session: "s1", root: "/w", items: [] });
    expect(useStore.getState().mediaUploads["/w"]).toEqual([]);
  });
});
