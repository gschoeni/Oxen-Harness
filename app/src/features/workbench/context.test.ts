import { beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";
import { loadFeatureFlags, sampleSession } from "../../test/ipcMock";
import { initFeatureFlags } from "../../lib/features";
import "../../modules";
beforeEach(async () => {
  resetAll();
  // Graph files only resolve to the workflow view while its release flag is on.
  loadFeatureFlags.mockResolvedValueOnce({ workbench_customization: false, workflows: true, advanced_settings: false });
  await initFeatureFlags();
});
describe("conversation-owned work contexts", () => {
  it("resolves graph files, keeps background events in their conversation, and retains navigation", () => {
    useStore.setState({ session: { ...sampleSession, session_id: "a" } });
    useStore.getState().ingestOpenFile({ session: "a", paths: ["studio.graph.json"] });
    useStore.getState().ingestOpenFile({ session: "b", paths: ["other.graph.json"] });
    expect(useStore.getState().workContexts.a.current).toMatchObject({
      view: "workflow",
      path: "studio.graph.json",
    });
    expect(useStore.getState().session?.session_id).toBe("a");
    useStore.getState().openWorkView("a", { view: "gallery" });
    useStore.getState().travelWorkView(-1);
    expect(useStore.getState().workContexts.a.current.path).toBe("studio.graph.json");
    expect(useStore.getState().workContexts.b.current.path).toBe("other.graph.json");
  });
  it("an agent's open expands a right column folded to its rail — for the chat on screen only", () => {
    useStore.setState({ session: { ...sampleSession, session_id: "a" }, dockCollapsed: { right: true } });
    useStore.getState().ingestOpenFile({ session: "b", paths: ["background.md"] });
    expect(useStore.getState().dockCollapsed.right).toBe(true);
    useStore.getState().ingestOpenFile({ session: "a", paths: ["picks/2026-09-25-week-3.md"] });
    expect(useStore.getState().dockCollapsed.right).toBe(false);
    expect(useStore.getState().workContexts.a.current.path).toBe("picks/2026-09-25-week-3.md");
  });
  it("opening a browser in one conversation never overwrites another's URL", () => {
    useStore.setState({ session: { ...sampleSession, session_id: "a" } });
    useStore.getState().openBrowser("https://example.com/a");
    useStore.setState({ session: { ...sampleSession, session_id: "b" } });
    useStore.getState().openBrowser("https://example.com/b");
    expect(useStore.getState().workContexts.a.current.url).toBe("https://example.com/a");
    expect(useStore.getState().workContexts.b.current.url).toBe("https://example.com/b");
  });
  it("keeps a gallery selection with its conversation", () => {
    useStore.setState({ session: { ...sampleSession, session_id: "a" } });
    useStore.getState().openGallery("image-a");
    useStore.setState({ session: { ...sampleSession, session_id: "b" } });
    useStore.getState().openGallery("image-b");
    expect(useStore.getState().workContexts.a.current.id).toBe("image-a");
    expect(useStore.getState().workContexts.b.current.id).toBe("image-b");
  });
});
