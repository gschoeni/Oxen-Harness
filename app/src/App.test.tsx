import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, waitFor } from "@testing-library/react";

vi.mock("./lib/ipc", () => import("./test/ipcMock"));

import App from "./App";
import { useStore } from "./lib/store";
import { resetAll } from "./test/utils";
import { sampleSession } from "./test/ipcMock";

beforeEach(() => {
  resetAll();
  useStore.setState({ dockWidths: {}, dockCollapsed: {} });
});
afterEach(() => vi.unstubAllGlobals());

it.each(["open_file", "open_view", "canvas", "preview"])(
  "shows an agent's %s request in a narrow window with default column widths", async (tool) => {
    vi.stubGlobal("innerWidth", 800);
    const { container } = render(<App />);
    const session = sampleSession.session_id;
    await waitFor(() => expect(useStore.getState().session?.session_id).toBe(session));
    await act(async () => {
      const state = useStore.getState();
      switch (tool) {
        case "open_file":
          state.ingestOpenFile({ session, paths: ["notes.txt"] });
          break;
        case "open_view":
          state.openWorkView(session, { view: "editor", path: "notes.txt" }, true);
          break;
        case "canvas":
          state.ingestCanvas({ session, id: "notes", title: "Notes", format: "markdown", content: "# Notes" });
          break;
        case "preview":
          state.ingestPreviewStatus({ session, phase: "ready", name: "dev", command: "npm run dev", url: "http://localhost:1430", port: 1430, message: null });
          break;
      }
    });
    expect(container.querySelector(".dock-column.right")).toBeInTheDocument();
    expect(container.querySelector(".dock-rail.right")).not.toBeInTheDocument();
    expect(container.querySelector(".chat-collapsed")).not.toBeInTheDocument();
  },
);

it("keeps the file column visible when both panels fit at their minimum widths", async () => {
  vi.stubGlobal("innerWidth", 1024);
  const { container } = render(<App />);
  await waitFor(() => expect(useStore.getState().session?.session_id).toBe(sampleSession.session_id));
  await act(async () => useStore.getState().openInViewer(["notes.txt"]));
  expect(container.querySelector(".dock-column.right")).toBeInTheDocument();
  expect(container.querySelector(".dock-column.left")).toBeInTheDocument();
});

describe("App overlays", () => {
  it("paints Settings above Home when both are open", () => {
    // They share a stacking band, so DOM order decides which one you see.
    // Settings opens *from* Home; rendering it first made the button
    // look dead — the surface mounted, entirely behind the page it opened from.
    useStore.setState({ homeOpen: true, settingsOpen: true });

    const { container } = render(<App />);
    const home = container.querySelector(".home-overlay");
    const settings = container.querySelector(".settings-overlay");

    expect(home).toBeTruthy();
    expect(settings).toBeTruthy();
    // Following in document order means painting on top at equal z-index.
    expect(home!.compareDocumentPosition(settings!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });
});
