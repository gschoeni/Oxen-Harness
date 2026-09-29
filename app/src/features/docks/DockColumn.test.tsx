import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../../lib/features", () => ({ workbenchCustomizationEnabled: vi.fn(() => false) }));
vi.mock("../files/EditorPane", () => ({
  EditorPane: () => {
    const pane = useStore((s) => s.editorTabs[s.session!.session_id]);
    return <p>Viewing {pane?.tabs[pane.active]?.join(", ")}</p>;
  },
}));
import { workbenchCustomizationEnabled } from "../../lib/features";

import { DockColumn } from "./DockColumn";
import { useStore } from "../../lib/store";
import { getUi } from "../../lib/uiState";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const previewReady = (session = "s1") =>
  act(() =>
    useStore.getState().ingestPreviewStatus({
      session,
      phase: "ready",
      name: "dev",
      command: "npm run dev",
      url: "http://localhost:5173",
      port: 5173,
      message: null,
    }),
  );

const openCanvas = () =>
  act(() =>
    useStore.getState().ingestCanvas({
      session: "s1",
      id: "report",
      title: "Report",
      format: "markdown",
      content: "# hi",
    }),
  );

beforeEach(() => {
  resetAll();
  vi.mocked(workbenchCustomizationEnabled).mockReturnValue(false);
  localStorage.clear();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
    homeOpen: false,
    dockWidths: {},
    dockCollapsed: {},
  });
});

describe("Dock columns", () => {
  it("the left dock collapses to a rail and comes back", async () => {
    render(<DockColumn side="left" />);
    // The Files dock is always available, so the column renders.
    expect(screen.getByRole("button", { name: "Collapse left panel" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Collapse left panel" }));
    expect(useStore.getState().dockCollapsed.left).toBe(true);
    // Collapsed: a rail with an expand button, no file tree.
    expect(screen.queryByRole("navigation", { name: "Project files" })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Expand left panel" }));
    expect(useStore.getState().dockCollapsed.left).toBe(false);
  });

  it("collapses to a rail without carrying the app mark", () => {
    // The brand mark left the rail (docks.tsx no longer wires a railHeader
    // for it); collapsing just yields the dock icons.
    const { rerender } = render(<DockColumn side="left" />);
    act(() => useStore.getState().setDockCollapsed("left", true));
    rerender(<DockColumn side="left" />);
    expect(screen.queryByText("🐂")).toBeNull();
    expect(screen.getByRole("button", { name: "Expand left panel" })).toBeInTheDocument();
  });

  it("remembers the collapsed state and width across runs", () => {
    act(() => useStore.getState().setDockCollapsed("left", true));
    act(() => useStore.getState().setDockWidth("right", 640));
    const saved = getUi("docks");
    expect(saved?.collapsed.left).toBe(true);
    expect(saved?.widths.right).toBe(640);
  });

  it("keeps an empty conversation free of a work panel or rail", () => {
    const { container } = render(<DockColumn side="right" />);
    expect(container).toBeEmptyDOMElement();
    act(() => useStore.getState().toggleDock("right"));
    expect(container).toBeEmptyDOMElement();
  });

  it("opens a clicked file and keeps new conversations closed", () => {
    const { container } = render(<DockColumn side="right" />);
    act(() => useStore.getState().openInViewer(["notes.txt"]));
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
    act(() => useStore.setState({ session: { ...ipc.sampleSession, session_id: "new-chat" } }));
    expect(container).toBeEmptyDOMElement();
    act(() => useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1" } }));
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
  });

  it("opens and reopens the visible chat's panel for the agent's open_view tool", () => {
    useStore.setState({ dockCollapsed: { right: true } });
    render(<DockColumn side="right" />);
    const open = () => useStore.getState().openWorkView("s1", { view: "editor", path: "notes.txt" }, true);
    act(open);
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
    act(() => useStore.getState().setDockCollapsed("right", true));
    expect(screen.queryByText("Viewing notes.txt")).not.toBeInTheDocument();
    act(open);
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
  });

  it("keeps a background agent's open out of the visible conversation", () => {
    const { container } = render(<DockColumn side="right" />);
    act(() => useStore.getState().ingestOpenFile({ session: "background", paths: ["result.txt"] }));
    expect(container).toBeEmptyDOMElement();
    act(() => useStore.setState({ session: { ...ipc.sampleSession, session_id: "background" } }));
    expect(screen.getByText("Viewing result.txt")).toBeInTheDocument();
  });

  it("does not open for saved history, filesystem changes or a preview status sync", async () => {
    const target = { view: "editor", path: "saved.txt" };
    useStore.setState({ workContexts: { s1: { current: target, history: [target], cursor: 0, pinned: false } } });
    const { container } = render(<DockColumn side="right" />);
    await act(async () => {
      useStore.getState().ingestFsChange({ root: ipc.sampleSession.workspace, paths: ["saved.txt"] });
      await useStore.getState().syncPreview("s1");
    });
    expect(container).toBeEmptyDOMElement();
  });

  it("does not show an empty panel for disabled customization history", () => {
    const target = { view: "view-studio", path: "views/example" };
    useStore.setState({
      workContexts: { s1: { current: target, history: [target], cursor: 0, pinned: false } },
      rightTab: { s1: "view-studio" },
    });
    const { container } = render(<DockColumn side="right" />);
    expect(container).toBeEmptyDOMElement();
  });

  it("uses one view picker without a second tab strip", async () => {
    vi.mocked(workbenchCustomizationEnabled).mockReturnValue(true);
    previewReady();
    const { rerender } = render(<DockColumn side="right" />);
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();

    openCanvas();
    rerender(<DockColumn side="right" />);
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox",{name:"Work view"})).toHaveTextContent("Canvas");

    // The same picker controls the conversation's current work surface.
    await userEvent.click(screen.getByRole("combobox", { name: "Work view" }));
    await userEvent.click(screen.getByRole("option", { name: "Preview" }));
    expect(useStore.getState().rightTab.s1).toBe("preview");
  });

  it("restores a package file through history without showing the previously active file", async () => {
    const history = [
      { view: "package:notes", path: "notes.txt" },
      { view: "editor", path: "other.txt" },
    ];
    useStore.setState({
      workContexts: { s1: { current: history[1], history, cursor: 1, pinned: false } },
      rightTab: { s1: "editor" },
      editorTabs: { s1: { tabs: [["other.txt"]], active: 0 } },
    });
    render(<DockColumn side="right" />);
    expect(screen.getByText("Viewing other.txt")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Previous work view" }));
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
    expect(useStore.getState().workContexts.s1.history).toEqual(history);
    await userEvent.click(screen.getByRole("button", { name: "Next work view" }));
    expect(screen.getByText("Viewing other.txt")).toBeInTheDocument();
  });

  it("keeps forward history when restoring a package file on startup", async () => {
    const history = [
      { view: "package:notes", path: "notes.txt" },
      { view: "editor", path: "other.txt" },
    ];
    useStore.setState({
      workContexts: { s1: { current: history[0], history, cursor: 0, pinned: false } },
      rightTab: { s1: "package:notes" },
    });
    render(<DockColumn side="right" />);
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
    expect(useStore.getState().workContexts.s1.history).toEqual(history);
    await userEvent.click(screen.getByRole("button", { name: "Next work view" }));
    expect(screen.getByText("Viewing other.txt")).toBeInTheDocument();
  });

  it("clicking a dock's icon in a collapsed rail expands the column onto it", async () => {
    previewReady();
    openCanvas();
    act(() => useStore.getState().setDockCollapsed("right", true));
    render(<DockColumn side="right" />);

    await userEvent.click(screen.getByRole("button", { name: "Open Work view" }));
    expect(useStore.getState().dockCollapsed.right).toBe(false);
    expect(useStore.getState().rightTab.s1).toBe("canvas");
  });
});
