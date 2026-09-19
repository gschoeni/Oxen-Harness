import { beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";
import { sampleSession } from "../../test/ipcMock";
import "../../modules";
beforeEach(resetAll);
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
});
