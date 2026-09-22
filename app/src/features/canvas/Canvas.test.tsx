import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Canvas } from "./Canvas";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

/** The project file's text, as the asset protocol would serve it. */
const disk = { text: "<h1>from disk</h1>" };
const fetchMock = vi.fn(async (url: string) => ({
  ok: true,
  statusText: "OK",
  text: async () => `${disk.text}<!-- ${url} -->`,
}));

beforeEach(() => {
  resetAll();
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockClear();
  useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" } });
});
afterEach(() => vi.unstubAllGlobals());

describe("Canvas", () => {
  it("shows a project file's current text and follows its edits", async () => {
    act(() =>
      useStore.getState().ingestCanvas({
        session: "s1",
        id: "site-index-html",
        title: "index.html",
        format: "html",
        language: null,
        content: "<h1>as sent</h1>",
        path: "site/index.html",
      }),
    );
    render(<Canvas />);
    const frame = () => screen.getByTitle("canvas document") as HTMLIFrameElement;
    // The call's content is the placeholder until the file is read.
    expect(frame().getAttribute("srcdoc")).toContain("as sent");
    await waitFor(() => expect(frame().getAttribute("srcdoc")).toContain("from disk"));
    expect(fetchMock.mock.calls[0][0]).toContain("asset://localhost//w/site/index.html");

    // The agent edits the file: the watcher batch names it, and the panel re-reads.
    disk.text = "<h1>edited on disk</h1>";
    act(() => useStore.getState().ingestFsChange({ root: "/w", paths: ["site/index.html"] }));
    await waitFor(() => expect(frame().getAttribute("srcdoc")).toContain("edited on disk"));
    expect(fetchMock.mock.calls[1][0]).toContain("?v=1");

    // A batch for other files is not a reason to re-read.
    act(() => useStore.getState().ingestFsChange({ root: "/w", paths: ["README.md"] }));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("renders a content doc without touching the disk", () => {
    act(() =>
      useStore.getState().ingestCanvas({
        session: "s1",
        id: "report",
        title: "Report",
        format: "html",
        language: null,
        content: "<h1>inline</h1>",
      }),
    );
    render(<Canvas />);
    expect((screen.getByTitle("canvas document") as HTMLIFrameElement).getAttribute("srcdoc")).toContain("inline");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
