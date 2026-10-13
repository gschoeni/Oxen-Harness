import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../../lib/features", () => ({
  workbenchCustomizationEnabled: vi.fn(() => false),
  workflowsEnabled: vi.fn(() => false),
}));
vi.mock("../files/EditorPane", () => ({
  EditorPane: () => {
    const pane = useStore((s) => s.editorTabs[s.session!.session_id]);
    return <p>Viewing {pane?.tabs[pane.active]?.join(", ")}</p>;
  },
}));
import { workbenchCustomizationEnabled } from "../../lib/features";

import { DockColumn } from "./DockColumn";
import { PanelSwitch } from "./PanelSwitch";
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

/** A column beside the title bar's switch, as the window renders them. */
const withSwitch = (side: "left" | "right", forceRail = false) => (
  <>
    <PanelSwitch />
    <DockColumn side={side} forceRail={forceRail} />
  </>
);
const workSwitch = () => screen.getByRole("button", { name: "Work" });

describe("Dock columns", () => {
  it("the switch hides the left dock entirely and brings it back", async () => {
    render(withSwitch("left"));
    expect(screen.getByRole("navigation", { name: "Project files" })).toBeInTheDocument();
    const files = screen.getByRole("button", { name: "Files" });
    expect(files).toHaveAttribute("aria-pressed", "true");

    await userEvent.click(files);
    expect(useStore.getState().dockCollapsed.left).toBe(true);
    // Hidden means gone: no file tree and no rail.
    expect(screen.queryByRole("navigation", { name: "Project files" })).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Collapsed left panel" })).toBeNull();

    await userEvent.click(files);
    expect(useStore.getState().dockCollapsed.left).toBe(false);
    expect(screen.getByRole("navigation", { name: "Project files" })).toBeInTheDocument();
  });

  it("folds to a rail only when the window squeezes it, and the rail brings it back", async () => {
    render(withSwitch("left", true));
    expect(screen.queryByRole("navigation", { name: "Project files" })).toBeNull();
    expect(screen.queryByText("🐂")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Open Files" }));
    expect(useStore.getState().dockCollapsed.left).toBe(false);
  });

  it("hides the chat and keeps the last panel showing", async () => {
    render(withSwitch("left"));
    await userEvent.click(screen.getByRole("button", { name: "Agent" }));
    expect(useStore.getState().dockCollapsed.chat).toBe(true);
    // Files is all that's left on screen (the work view has nothing open),
    // so it can't be switched off.
    const files = screen.getByRole("button", { name: "Files" });
    expect(files).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Agent" }));
    expect(useStore.getState().dockCollapsed.chat).toBe(false);
    expect(files).toBeEnabled();
  });

  it("remembers the collapsed state and width across runs", () => {
    act(() => useStore.getState().setDockCollapsed("left", true));
    act(() => useStore.getState().setDockWidth("right", 640));
    const saved = getUi("docks");
    expect(saved?.collapsed.left).toBe(true);
    expect(saved?.widths.right).toBe(640);
  });

  it("keeps an empty conversation's work view hidden until the switch shows it", async () => {
    render(withSwitch("right"));
    expect(screen.queryByRole("region", { name: "Current work" })).toBeNull();
    expect(workSwitch()).toHaveAttribute("aria-pressed", "false");

    // Switching it on opens the pane on its empty state, and that counts as
    // this chat having opened it: it stays until switched off again.
    await userEvent.click(workSwitch());
    expect(screen.getByRole("region", { name: "Current work" })).toBeInTheDocument();
    expect(useStore.getState().rightTab.s1).toBe("welcome");
    await userEvent.click(workSwitch());
    expect(screen.queryByRole("region", { name: "Current work" })).toBeNull();
    expect(workSwitch()).toHaveAttribute("aria-pressed", "false");
  });

  it("opens a clicked file and keeps new conversations' work view hidden", () => {
    render(withSwitch("right"));
    act(() => useStore.getState().openInViewer(["notes.txt"]));
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
    act(() => useStore.setState({ session: { ...ipc.sampleSession, session_id: "new-chat" } }));
    expect(screen.queryByText("Viewing notes.txt")).toBeNull();
    expect(workSwitch()).toHaveAttribute("aria-pressed", "false");
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
    render(withSwitch("right"));
    act(() => useStore.getState().ingestOpenFile({ session: "background", paths: ["result.txt"] }));
    expect(screen.queryByText("Viewing result.txt")).toBeNull();
    expect(workSwitch()).toHaveAttribute("aria-pressed", "false");
    act(() => useStore.setState({ session: { ...ipc.sampleSession, session_id: "background" } }));
    expect(screen.getByText("Viewing result.txt")).toBeInTheDocument();
  });

  it("does not open for saved history, filesystem changes or a preview status sync", async () => {
    const target = { view: "editor", path: "saved.txt" };
    useStore.setState({ workContexts: { s1: { current: target, history: [target], cursor: 0 } } });
    render(withSwitch("right"));
    await act(async () => {
      useStore.getState().ingestFsChange({ root: ipc.sampleSession.workspace, paths: ["saved.txt"] });
      await useStore.getState().syncPreview("s1");
    });
    expect(screen.queryByText("Viewing saved.txt")).toBeNull();
    expect(workSwitch()).toHaveAttribute("aria-pressed", "false");
  });

  it("brings saved history back when shown instead of an empty pane", async () => {
    const target = { view: "editor", path: "saved.txt" };
    useStore.setState({ workContexts: { s1: { current: target, history: [target], cursor: 0 } } });
    render(withSwitch("right"));
    await userEvent.click(workSwitch());
    expect(screen.getByText("Viewing saved.txt")).toBeInTheDocument();
    expect(useStore.getState().workContexts.s1.history).toEqual([target]);
  });

  it("does not show an empty panel for disabled customization history", () => {
    const target = { view: "view-studio", path: "views/example" };
    useStore.setState({
      workContexts: { s1: { current: target, history: [target], cursor: 0 } },
      rightTab: { s1: "view-studio" },
    });
    render(withSwitch("right"));
    expect(screen.queryByRole("region", { name: "Current work" })).toBeNull();
    expect(workSwitch()).toHaveAttribute("aria-pressed", "false");
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
      workContexts: { s1: { current: history[1], history, cursor: 1 } },
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
      workContexts: { s1: { current: history[0], history, cursor: 0 } },
      rightTab: { s1: "package:notes" },
    });
    render(<DockColumn side="right" />);
    expect(screen.getByText("Viewing notes.txt")).toBeInTheDocument();
    expect(useStore.getState().workContexts.s1.history).toEqual(history);
    await userEvent.click(screen.getByRole("button", { name: "Next work view" }));
    expect(screen.getByText("Viewing other.txt")).toBeInTheDocument();
  });

  it("showing a hidden work view brings back what it had open", async () => {
    previewReady();
    openCanvas();
    act(() => useStore.getState().setDockCollapsed("right", true));
    render(withSwitch("right"));

    await userEvent.click(workSwitch());
    expect(useStore.getState().dockCollapsed.right).toBe(false);
    expect(useStore.getState().rightTab.s1).toBe("canvas");
  });
});
