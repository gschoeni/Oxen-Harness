import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { DockColumn } from "./DockColumn";
import { useStore } from "../../lib/store";
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
  localStorage.clear();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
    projectsOpen: false,
    dockWidths: {},
    dockCollapsed: {},
  });
});

describe("Dock columns", () => {
  it("the left dock collapses to a rail and comes back", async () => {
    render(<DockColumn side="left" />);
    // The chat list is always available, so the column renders.
    expect(screen.getByRole("button", { name: "Collapse left panel" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Collapse left panel" }));
    expect(useStore.getState().dockCollapsed.left).toBe(true);
    // Collapsed: a rail with an expand button, no chat list.
    expect(screen.queryByText("New chat")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Expand left panel" }));
    expect(useStore.getState().dockCollapsed.left).toBe(false);
  });

  it("keeps the app's mark visible in the collapsed rail", () => {
    // The logo holds its spot in both states, so the top of the column doesn't
    // lurch when it collapses. (The shared top inset that lines the two up is
    // CSS — jsdom has no layout, so only presence is asserted here.)
    const { rerender } = render(<DockColumn side="left" />);
    expect(screen.getByText("🐂")).toBeInTheDocument();

    act(() => useStore.getState().setDockCollapsed("left", true));
    rerender(<DockColumn side="left" />);
    expect(screen.getByText("🐂")).toBeInTheDocument();
  });

  it("remembers the collapsed state and width across runs", () => {
    act(() => useStore.getState().setDockCollapsed("left", true));
    act(() => useStore.getState().setDockWidth("right", 640));
    const saved = JSON.parse(localStorage.getItem("oxen-docks") ?? "{}");
    expect(saved.collapsed.left).toBe(true);
    expect(saved.widths.right).toBe(640);
  });

  it("a side with nothing docked renders no column", () => {
    // No dev server, no canvas → the right side has no docks at all.
    const { container } = render(<DockColumn side="right" />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows tabs only when a side has more than one dock with content", async () => {
    previewReady();
    const { rerender } = render(<DockColumn side="right" />);
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();

    openCanvas();
    rerender(<DockColumn side="right" />);
    expect(screen.getByRole("tab", { name: /Preview/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Canvas/ })).toBeInTheDocument();

    // The canvas took the panel when it opened; switching back is one click.
    await userEvent.click(screen.getByRole("tab", { name: /Preview/ }));
    expect(useStore.getState().rightTab.s1).toBe("preview");
  });

  it("clicking a dock's icon in a collapsed rail expands the column onto it", async () => {
    previewReady();
    openCanvas();
    act(() => useStore.getState().setDockCollapsed("right", true));
    render(<DockColumn side="right" />);

    await userEvent.click(screen.getByRole("button", { name: "Open Preview" }));
    expect(useStore.getState().dockCollapsed.right).toBe(false);
    expect(useStore.getState().rightTab.s1).toBe("preview");
  });
});
