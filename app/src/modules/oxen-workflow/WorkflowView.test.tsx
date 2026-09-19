import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkflowView } from "./WorkflowView";
import type { WorkbenchAPI } from "../../workbench-sdk";
import { parseGraph } from "./graph";

describe("workflow creation", () => {
  it("creates a portable graph and opens it without running any models", async () => {
    const save = vi.fn(async (path, content) => ({ path, content, revision: "one" }));
    const open = vi.fn(),
      request = vi.fn();
    const api = {
      context: { session: "s", workspace: "project", target: { view: "workflow" } },
      save,
      open,
      request,
    } as unknown as WorkbenchAPI;
    render(<WorkflowView api={api} />);
    await userEvent.click(screen.getByRole("button", { name: /Image studio/ }));
    expect(save).toHaveBeenCalledOnce();
    const graph = parseGraph(save.mock.calls[0][1]);
    expect(graph.nodes.map((n) => n.kind)).toEqual([
      "prompt",
      "rewrite",
      "image",
      "upscale",
      "output",
    ]);
    expect(open).toHaveBeenCalledWith({ view: "workflow", path: "workflows/studio.graph.json" });
    expect(request).not.toHaveBeenCalled();
  });
  it("shows a create conflict without replacing an existing graph", async () => {
    const api = {
      context: { session: "s", workspace: "project", target: { view: "workflow" } },
      save: vi.fn().mockRejectedValue(new Error("file already exists")),
      open: vi.fn(),
    } as unknown as WorkbenchAPI;
    render(<WorkflowView api={api} />);
    await userEvent.click(screen.getByRole("button", { name: /Start empty/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("file already exists");
    expect(api.open).not.toHaveBeenCalled();
  });
});
