import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { act, cleanup, render, renderHook, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Workflow } from "lucide-react";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../../modules", () => ({}));
vi.mock("../../lib/features", () => ({
  workbenchCustomizationEnabled: vi.fn(() => false),
  workflowsEnabled: vi.fn(() => false),
}));
vi.mock("./packages", () => ({ refreshPackages: vi.fn(async () => []) }));
const api = vi.hoisted(() => ({ open: vi.fn(), report: vi.fn() }));
vi.mock("./api", () => ({ useWorkbenchAPI: () => api }));
import { Workbench as WorkbenchPanel } from "./Workbench";
import { useWorkbenchRegistration } from "./useWorkbenchRegistration";
import { registerView, removeView, views, resolveView } from "./registry";
import { workbenchCustomizationEnabled } from "../../lib/features";
import { refreshPackages } from "./packages";
import { useStore } from "../../lib/store";
import { sampleSession, workbenchRequest } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

function Workbench() {
  useWorkbenchRegistration();
  return <WorkbenchPanel />;
}

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
    const target = { view };
    useStore.setState({ workContexts: {
      [sampleSession.session_id]: { current: target, history: [target], cursor: 0 },
    } });
    render(<Workbench />);
    expect(screen.queryByText("Customization mounted")).not.toBeInTheDocument();
    expect(screen.getByText("Open a file to get started. This space follows your conversation.")).toBeInTheDocument();
    expect(views().some((module) => module.id === view)).toBe(false);
    expect(resolveView("studio.graph.json")).toBe("graph");
  },
);

it("falls back to a bundled file viewer for a previously installed view", () => {
  const target = { view: "package:demo", path: "notes.txt" };
  useStore.setState({ workContexts: {
    [sampleSession.session_id]: { current: target, history: [target], cursor: 0 },
  } });
  render(<Workbench />);
  expect(screen.getByText("File contents")).toBeInTheDocument();
});

it("restores the customization entry points when opted in", async () => {
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
  registerView({
    id: "view-studio", title: "View Studio", description: "Build a view.",
    component: () => null,
  });
  render(<Workbench />);
  expect(refreshPackages).toHaveBeenCalledOnce();
  expect(screen.getByRole("button", { name: "View Studio Build a view." })).toBeInTheDocument();
  await userEvent.click(screen.getByRole("combobox", { name: "Work view" }));
  await userEvent.click(screen.getByRole("option", { name: "View Studio" }));
  expect(api.open).toHaveBeenCalledWith({ view: "view-studio", path: undefined, paths: undefined });
});

it("loads packages once after a session exists, independently of opening a panel", () => {
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
  useStore.setState({ session: null });
  renderHook(useWorkbenchRegistration);
  expect(refreshPackages).not.toHaveBeenCalled();
  act(() => useStore.setState({ session: sampleSession }));
  expect(refreshPackages).toHaveBeenCalledOnce();
  act(() => useStore.setState({ session: { ...sampleSession, session_id: "other" } }));
  expect(refreshPackages).toHaveBeenCalledOnce();
});

it("keeps discovery failures with their initiating conversation after a tab switch", async () => {
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
  let rejectPackages!: (reason: Error) => void;
  let rejectRegistration!: (reason: Error) => void;
  vi.mocked(refreshPackages).mockImplementationOnce(() => new Promise((_, reject) => { rejectPackages = reject; }));
  workbenchRequest.mockImplementationOnce(() => new Promise((_, reject) => { rejectRegistration = reject; }));
  useStore.setState({ threads: { [sampleSession.session_id]: [], other: [] } });
  renderHook(useWorkbenchRegistration);
  act(() => useStore.setState({ session: { ...sampleSession, session_id: "other" } }));
  await act(async () => {
    rejectPackages(new Error("package unavailable"));
    rejectRegistration(new Error("registration unavailable"));
  });
  const state = useStore.getState();
  expect(state.threads[sampleSession.session_id]).toEqual(expect.arrayContaining([
    expect.objectContaining({ kind: "notice", text: "Load installed views: Error: package unavailable" }),
    expect.objectContaining({ kind: "notice", text: "Register work views: Error: registration unavailable" }),
  ]));
  expect(state.threads.other).toEqual([]);
});
