// The model's `open_file` tool → `agent://open-file` → ingestOpenFile: the
// session's Editor/viewer dock shows the file and its tab comes to the front,
// without any cross-session side effects.

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";

beforeEach(resetAll);

describe("ingestOpenFile", () => {
  it("opens the paths in that session's editor and fronts its tab", () => {
    useStore.getState().ingestOpenFile({ session: "s1", paths: ["src/main.rs"] });
    expect(useStore.getState().editorTabs.s1).toEqual({ tabs: [["src/main.rs"]], active: 0 });
    expect(useStore.getState().rightTab.s1).toBe("editor");
  });

  it("keys strictly by session — a background chat can't touch another's pane", () => {
    useStore.getState().ingestOpenFile({ session: "s1", paths: ["a.md"] });
    useStore.getState().ingestOpenFile({ session: "s2", paths: ["b.md"] });
    expect(useStore.getState().editorTabs.s1).toEqual({ tabs: [["a.md"]], active: 0 });
    expect(useStore.getState().editorTabs.s2).toEqual({ tabs: [["b.md"]], active: 0 });
  });

  it("accumulates tabs and fronts an already-open one instead of duplicating", () => {
    useStore.getState().ingestOpenFile({ session: "s1", paths: ["a.md"] });
    useStore.getState().ingestOpenFile({ session: "s1", paths: ["b.md"] });
    expect(useStore.getState().editorTabs.s1).toEqual({ tabs: [["a.md"], ["b.md"]], active: 1 });
    useStore.getState().ingestOpenFile({ session: "s1", paths: ["a.md"] });
    expect(useStore.getState().editorTabs.s1).toEqual({ tabs: [["a.md"], ["b.md"]], active: 0 });
  });

  it("ignores an empty path list", () => {
    useStore.getState().ingestOpenFile({ session: "s1", paths: [] });
    expect(useStore.getState().editorTabs.s1).toBeUndefined();
    expect(useStore.getState().rightTab.s1).toBeUndefined();
  });
});
