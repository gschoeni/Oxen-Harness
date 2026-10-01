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
  it("a pinned work view says when it kept an agent's open out of sight", () => {
    useStore.setState({ session: { ...sampleSession, session_id: "a" }, threads: { a: [] } });
    const s = useStore.getState();
    s.openWorkView("a", { view: "gallery" });
    s.pinWorkView();
    s.ingestOpenFile({ session: "a", paths: ["notes.md"] });
    const thread = useStore.getState().threads.a;
    const last = thread[thread.length - 1];
    expect(last.kind).toBe("notice");
    expect(last.kind === "notice" && last.text).toContain("notes.md");
    expect(useStore.getState().workContexts.a.current.view).toBe("gallery");
  });
  it("a pin stops agent focus changes but allows the user's explicit navigation", () => {
    useStore.setState({ session: { ...sampleSession, session_id: "a" } });
    const s = useStore.getState();
    s.openWorkView("a", { view: "workflow", path: "a.graph.json" });
    s.pinWorkView();
    s.openWorkView("a", { view: "editor", path: "background.md" }, true);
    expect(useStore.getState().workContexts.a.current.view).toBe("workflow");
    s.openWorkView("a", { view: "gallery" });
    expect(useStore.getState().workContexts.a.current.view).toBe("gallery");
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
