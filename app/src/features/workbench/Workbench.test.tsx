import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Workflow } from "lucide-react";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../../modules", () => ({}));
vi.mock("../../lib/features", () => ({ workbenchCustomizationEnabled: vi.fn(() => false) }));
vi.mock("./packages", () => ({ refreshPackages: vi.fn(async () => []) }));
const api = vi.hoisted(() => ({ open: vi.fn(), report: vi.fn() }));
vi.mock("./api", () => ({ useWorkbenchAPI: () => api }));
import { Workbench } from "./Workbench";
import { registerView, removeView, views, resolveView } from "./registry";
import { workbenchCustomizationEnabled } from "../../lib/features";
import { refreshPackages } from "./packages";
import { useStore } from "../../lib/store";
import { sampleSession } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(false);
  vi.mocked(refreshPackages).mockClear();
  api.open.mockReset();
  api.report.mockReset();
  useStore.setState({ session: sampleSession });
  registerView({
    id: "file",
    title: "File",
    description: "Edit files.",
    component: () => <p>File contents</p>,
    matches: () => true,
  });
  registerView({
    id: "graph",
    title: "Graph",
    description: "Connect your generation nodes.",
    icon: Workflow,
    component: () => <p>Graph contents</p>,
    matches: (path) => path.endsWith(".graph.json"),
    priority: 100,
  });
});
afterEach(() => {
  cleanup();
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
  views().forEach((view) => removeView(view.id));
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(false);
});

it("uses module icons and descriptions while preserving compatible resources", async () => {
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
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
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
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

it("keeps file-type rendering without the picker or customization cards", () => {
  const { rerender } = render(<Workbench />);
  expect(screen.queryByRole("combobox", { name: "Work view" })).not.toBeInTheDocument();
  expect(screen.getByText("Open a file to get started. This space follows your conversation.")).toBeInTheDocument();
  expect(document.querySelector(".workbench-choices")).toBeNull();
  expect(refreshPackages).not.toHaveBeenCalled();
  act(() => useStore.getState().openWorkView(sampleSession.session_id, {
    view: resolveView("studio.graph.json"), path: "studio.graph.json",
  }));
  rerender(<Workbench />);
  expect(screen.getByText("Graph contents")).toBeInTheDocument();
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
});

it.each(["view-studio", "view-manager", "package:demo"])(
  "does not restore disabled customization from history: %s", (view) => {
    registerView({
      id: view, title: "Custom view", description: "Authoring",
      component: () => <p>Customization mounted</p>,
      matches: () => true, priority: 200,
    });
    useStore.getState().openWorkView(sampleSession.session_id, { view });
    render(<Workbench />);
    expect(screen.queryByText("Customization mounted")).not.toBeInTheDocument();
    expect(screen.getByText("Open a file to get started. This space follows your conversation.")).toBeInTheDocument();
    expect(views().some((module) => module.id === view)).toBe(false);
    expect(resolveView("studio.graph.json")).toBe("graph");
  },
);

it("falls back to a bundled file viewer for a previously installed view", () => {
  useStore.getState().openWorkView(sampleSession.session_id, {
    view: "package:demo", path: "notes.txt",
  });
  render(<Workbench />);
  expect(screen.getByText("File contents")).toBeInTheDocument();
});
