import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Workflow } from "lucide-react";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../../modules", () => ({}));
vi.mock("./packages", () => ({ refreshPackages: vi.fn(async () => []) }));
const api = vi.hoisted(() => ({ open: vi.fn(), report: vi.fn() }));
vi.mock("./api", () => ({ useWorkbenchAPI: () => api }));
import { Workbench } from "./Workbench";
import { registerView, removeView, views } from "./registry";
import { useStore } from "../../lib/store";
import { sampleSession } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  api.open.mockReset();
  api.report.mockReset();
  useStore.setState({ session: sampleSession });
  registerView({
    id: "file",
    title: "File",
    description: "Edit files.",
    component: () => null,
    matches: () => true,
  });
  registerView({
    id: "graph",
    title: "Graph",
    description: "Connect your generation nodes.",
    icon: Workflow,
    component: () => null,
    matches: (path) => path.endsWith(".graph.json"),
  });
});
afterEach(() => {
  cleanup();
  views().forEach((view) => removeView(view.id));
});

it("uses module icons and descriptions while preserving compatible resources", async () => {
  useStore.getState().openWorkView(sampleSession.session_id, {
    view: "file",
    path: "studio.graph.json",
    paths: ["studio.graph.json", "notes.txt"],
  });
  render(<Workbench />);
  await userEvent.click(screen.getByRole("combobox", { name: "Work view" }));
  const graph = screen.getByRole("option", { name: "Graph" });
  expect(graph).toHaveAccessibleDescription("Connect your generation nodes.");
  expect(graph.querySelector(".lucide-workflow")).not.toBeNull();
  expect(
    screen
      .getByRole("option", { name: "File" })
      .querySelector(".lucide-panels-top-left"),
  ).not.toBeNull();
  await userEvent.click(graph);
  expect(api.open).toHaveBeenCalledWith({
    view: "graph",
    path: "studio.graph.json",
    paths: ["studio.graph.json"],
  });
});

it("drops a resource that does not belong to the chosen view", async () => {
  useStore.getState().openWorkView(sampleSession.session_id, {
    view: "file",
    path: "notes.txt",
  });
  render(<Workbench />);
  await userEvent.click(screen.getByRole("combobox", { name: "Work view" }));
  await userEvent.click(screen.getByRole("option", { name: "Graph" }));
  expect(api.open).toHaveBeenCalledWith({
    view: "graph",
    path: undefined,
    paths: undefined,
  });
});
