import { describe, expect, it } from "vitest";
import { newestFirst, searchChatModels } from "./modelSearch";
import { sampleOxenHits } from "../test/ipcMock";
import type { OxenModelHit } from "./types";

const hit = (id: string, released_at: string | null): OxenModelHit => ({
  ...sampleOxenHits[0],
  id,
  name: id,
  released_at,
});

describe("newestFirst", () => {
  it("leads with the latest release and leaves undated models last, by name", () => {
    const ordered = newestFirst([
      hit("b-undated", null),
      hit("old", "2025-01-10"),
      hit("a-undated", null),
      hit("new", "2026-09-29"),
    ]);
    expect(ordered.map((h) => h.id)).toEqual(["new", "old", "a-undated", "b-undated"]);
  });

  it("does not reorder the listing it was given", () => {
    const listing = [hit("old", "2025-01-10"), hit("new", "2026-09-29")];
    newestFirst(listing);
    expect(listing.map((h) => h.id)).toEqual(["old", "new"]);
  });
});

describe("searchChatModels", () => {
  it("keeps only chat models when the endpoint annotates routes", () => {
    expect(searchChatModels(sampleOxenHits, "").map((h) => h.id)).toEqual([
      "claude-sonnet-4-6",
      "muse-spark-1-1",
    ]);
  });
});
