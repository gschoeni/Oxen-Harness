import { afterAll, describe, expect, it, vi } from "vitest";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { initFeatureFlags } from "../../lib/features";
import { loadFeatureFlags } from "../../test/ipcMock";
import { availableTarget, resolveView, viewById, views } from "./registry";
import "../../modules";

async function setWorkflows(on: boolean) {
  loadFeatureFlags.mockResolvedValueOnce({
    workbench_customization: false,
    workflows: on,
    advanced_settings: false,
  });
  await initFeatureFlags();
}

afterAll(() => setWorkflows(false));

describe("the workflow view's release flag", () => {
  it("off: graph files open in the editor and the view is nowhere on offer", async () => {
    await setWorkflows(false);
    expect(views().some((view) => view.id === "workflow")).toBe(false);
    expect(viewById("workflow")).toBeUndefined();
    expect(resolveView("studio.graph.json")).toBe("editor");
    // A graph opened while the flag was on is still a file worth showing.
    expect(availableTarget({ view: "workflow", path: "studio.graph.json" })).toEqual({
      view: "editor",
      path: "studio.graph.json",
    });
    expect(availableTarget({ view: "workflow" })).toEqual({ view: "welcome" });
  });

  it("on: graph files open in the node graph", async () => {
    await setWorkflows(true);
    expect(viewById("workflow")?.title).toBe("Oxen workflow");
    expect(resolveView("studio.graph.json")).toBe("workflow");
    expect(availableTarget({ view: "workflow", path: "studio.graph.json" }).view).toBe("workflow");
  });
});
