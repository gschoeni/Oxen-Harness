import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Browser } from "./Browser";
import { startLinkRouting } from "../../lib/links";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    // resetAll boots on the Projects page, which counts as an overlay the
    // native webview must hide under — close it so the pane can attach.
    projectsOpen: false,
  });
});

describe("link routing (clicks must never navigate the main webview)", () => {
  let stop: () => void;
  beforeEach(() => {
    stop = startLinkRouting();
  });
  afterEach(() => stop());

  /** Render a link and click it, reporting whether default was prevented. */
  async function clickLink(html: string): Promise<boolean> {
    const host = document.createElement("div");
    host.innerHTML = html;
    document.body.appendChild(host);
    let prevented = false;
    // Record the verdict at the end of the bubble phase, then always cancel:
    // jsdom would otherwise log a "not implemented: navigation" error.
    const tail = (e: MouseEvent) => {
      prevented = e.defaultPrevented;
      e.preventDefault();
    };
    document.addEventListener("click", tail);
    await userEvent.click(host.querySelector("a")!);
    document.removeEventListener("click", tail);
    host.remove();
    return prevented;
  }

  it("a chat link opens in the side-panel browser, not the main webview", async () => {
    const prevented = await clickLink('<a href="https://example.com/docs">docs</a>');
    expect(prevented).toBe(true);
    const s = useStore.getState();
    expect(s.browserUrl).toBe("https://example.com/docs");
    expect(s.rightTab.s1).toBe("browser");
  });

  it("a target=_blank link goes to the system browser instead", async () => {
    const prevented = await clickLink(
      '<a href="https://hub.oxen.ai/settings" target="_blank" rel="noreferrer">credits</a>',
    );
    expect(prevented).toBe(true);
    expect(ipc.openExternal).toHaveBeenCalledWith("https://hub.oxen.ai/settings");
    expect(useStore.getState().browserUrl).toBeNull();
  });

  it("a mailto link goes to the system handler", async () => {
    const prevented = await clickLink('<a href="mailto:hi@oxen.ai">mail</a>');
    expect(prevented).toBe(true);
    expect(ipc.openExternal).toHaveBeenCalledWith("mailto:hi@oxen.ai");
  });

  it("in-page fragment links are left alone", async () => {
    const prevented = await clickLink('<a href="#section">jump</a>');
    expect(prevented).toBe(false);
    expect(useStore.getState().browserUrl).toBeNull();
  });

  it("the backend nav-guard's bounce event opens the panel too", () => {
    act(() => ipc.emit("browserOpen", "https://example.com/slipped-through"));
    expect(useStore.getState().browserUrl).toBe("https://example.com/slipped-through");
  });
});

describe("Browser store", () => {
  it("openBrowser claims the right-panel tab, closeBrowser releases the pane", () => {
    act(() => useStore.getState().openBrowser("https://example.com"));
    let s = useStore.getState();
    expect(s.browserUrl).toBe("https://example.com");
    expect(s.rightTab.s1).toBe("browser");

    act(() => useStore.getState().closeBrowser());
    s = useStore.getState();
    expect(s.browserUrl).toBeNull();
  });

  it("openBrowser expands a collapsed right column (the click must show)", () => {
    act(() => useStore.getState().setDockCollapsed("right", true));
    act(() => useStore.getState().openBrowser("https://example.com"));
    expect(useStore.getState().dockCollapsed.right).toBe(false);
  });
});

describe("Browser pane", () => {
  it("shows the URL and attaches the native webview to the placeholder", async () => {
    act(() => useStore.getState().openBrowser("https://example.com/docs"));
    render(<Browser />);
    expect(screen.getByText("https://example.com/docs")).toBeInTheDocument();
    await vi.waitFor(() => expect(ipc.browserAttach).toHaveBeenCalled());
    const [url, bounds] = ipc.browserAttach.mock.calls[0] as unknown as [
      string,
      { width: number },
    ];
    expect(url).toBe("https://example.com/docs");
    expect(bounds).toHaveProperty("width");
  });

  it("toolbar drives reload / pop-out / close", async () => {
    act(() => useStore.getState().openBrowser("https://example.com"));
    render(<Browser />);
    await userEvent.click(screen.getByRole("button", { name: "Reload page" }));
    expect(ipc.browserReload).toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Open in browser" }));
    expect(ipc.openExternal).toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Close browser pane" }));
    expect(ipc.browserClose).toHaveBeenCalled();
    expect(useStore.getState().browserUrl).toBeNull();
  });

  it("detaches the native webview while an overlay is open", async () => {
    act(() => useStore.getState().openBrowser("https://example.com"));
    render(<Browser />);
    await vi.waitFor(() => expect(ipc.browserAttach).toHaveBeenCalled());
    ipc.browserDetach.mockClear();

    act(() => useStore.getState().setSettingsOpen(true));
    await vi.waitFor(() => expect(ipc.browserDetach).toHaveBeenCalled());
    expect(screen.getByText(/Page hidden while a panel is open/)).toBeInTheDocument();
  });

  it("renders nothing when no link is open", () => {
    const { container } = render(<Browser />);
    expect(container).toBeEmptyDOMElement();
    expect(ipc.browserAttach).not.toHaveBeenCalled();
  });
});
