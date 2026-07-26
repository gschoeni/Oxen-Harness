import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

vi.mock("./lib/ipc", () => import("./test/ipcMock"));

import App from "./App";
import { useStore } from "./lib/store";
import { resetAll } from "./test/utils";

beforeEach(() => {
  resetAll();
});

describe("App overlays", () => {
  it("paints Settings above the Ledger when both are open", () => {
    // They share a stacking band, so DOM order decides which one you see.
    // Settings opens *from* the Ledger; rendering it first made the button
    // look dead — the surface mounted, entirely behind the page it opened from.
    useStore.setState({ homeOpen: true, settingsOpen: true });

    const { container } = render(<App />);
    const ledger = container.querySelector(".home-overlay");
    const settings = container.querySelector(".settings-overlay");

    expect(ledger).toBeTruthy();
    expect(settings).toBeTruthy();
    // Following in document order means painting on top at equal z-index.
    expect(ledger!.compareDocumentPosition(settings!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });
});
