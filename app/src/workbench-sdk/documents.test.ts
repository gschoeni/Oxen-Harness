import { describe, expect, it } from "vitest";
import { DocumentStore } from "./documents";
import type { WorkbenchAPI } from ".";

describe("shared document drafts", () => {
  it("retains edits across views and forces conflict resolution before overwrite", async () => {
    const store = new DocumentStore();
    let disk = { path: "a", content: "original", revision: "1" };
    const api = {
      context: { workspace: "project" },
      read: async () => disk,
      save: async (_: string, content: string) => (disk = { ...disk, content, revision: "3" }),
    } as unknown as WorkbenchAPI;
    await store.load(api, "a");
    const key = store.key(api, "a");
    store.edit(key, "draft");
    disk = { ...disk, content: "agent edit", revision: "2" };
    await store.load(api, "a");
    expect(store.get(key).content).toBe("draft");
    expect(store.get(key).conflict?.content).toBe("agent edit");
    await expect(store.save(api, "a")).rejects.toThrow("Resolve the conflict");
    store.resolve(key, "draft");
    await store.save(api, "a");
    expect(disk.content).toBe("draft");
  });
  it("does not mark edits made during a save as saved", async () => {
    const store = new DocumentStore();
    let finish!: (v: { path: string; content: string; revision: string }) => void;
    const api = {
      context: { workspace: "project" },
      read: async () => ({ path: "a", content: "one", revision: "1" }),
      save: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    } as unknown as WorkbenchAPI;
    await store.load(api, "a");
    const key = store.key(api, "a");
    store.edit(key, "two");
    const saving = store.save(api, "a");
    store.edit(key, "three");
    finish({ path: "a", content: "two", revision: "2" });
    await saving;
    expect(store.get(key).content).toBe("three");
    expect(store.get(key).dirty).toBe(true);
  });
});
