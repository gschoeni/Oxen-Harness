import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { FilesPanel } from "./FilesPanel";
import { fsCreateEntry, fsListDir, gitStatus, sampleSession } from "../../test/ipcMock";
import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";

const ROOT = sampleSession.workspace;

function seedSession() {
  useStore.setState({ session: sampleSession });
}

beforeEach(() => {
  resetAll();
  fsListDir.mockImplementation(async (_root: string, path: string) => {
    if (path === "") {
      return [
        { name: "src", path: "src", is_dir: true },
        { name: "README.md", path: "README.md", is_dir: false },
        { name: "logo.png", path: "logo.png", is_dir: false },
        { name: "hero.png", path: "hero.png", is_dir: false },
      ];
    }
    if (path === "src") return [{ name: "main.rs", path: "src/main.rs", is_dir: false }];
    return [];
  });
});

describe("FilesPanel", () => {
  it("lists the workspace root and lazily expands directories", async () => {
    seedSession();
    render(<FilesPanel />);
    expect(await screen.findByText("README.md")).toBeInTheDocument();
    expect(fsListDir).toHaveBeenCalledWith(ROOT, "");
    expect(screen.queryByText("main.rs")).not.toBeInTheDocument();

    await userEvent.click(screen.getByText("src"));
    expect(await screen.findByText("main.rs")).toBeInTheDocument();
    expect(fsListDir).toHaveBeenCalledWith(ROOT, "src");
  });

  it("opens a clicked file in the Editor pane (and fronts its tab)", async () => {
    seedSession();
    render(<FilesPanel />);
    await userEvent.click(await screen.findByText("README.md"));
    const id = sampleSession.session_id;
    expect(useStore.getState().editorTabs[id]).toEqual({ tabs: [["README.md"]], active: 0 });
    expect(useStore.getState().rightTab[id]).toBe("editor");
  });

  it("opens a ⌘-selected group of images as a gallery", async () => {
    seedSession();
    render(<FilesPanel />);
    const user = userEvent.setup();
    await user.click(await screen.findByText("logo.png"));
    await user.keyboard("{Meta>}");
    await user.click(screen.getByText("hero.png"));
    await user.keyboard("{/Meta}");
    const id = sampleSession.session_id;
    // The first click opened logo.png alone; the ⌘-click adds a gallery tab.
    expect(useStore.getState().editorTabs[id]).toEqual({
      tabs: [["logo.png"], ["logo.png", "hero.png"]],
      active: 1,
    });
  });

  it("creates a new file in the selected directory and opens it", async () => {
    seedSession();
    render(<FilesPanel />);
    await screen.findByText("README.md");
    await userEvent.click(screen.getByText("src")); // select + expand src
    await screen.findByText("main.rs");
    await userEvent.click(screen.getByRole("button", { name: "New file" }));
    const input = screen.getByLabelText("New file name");
    await userEvent.type(input, "lib.rs{Enter}");
    await waitFor(() => expect(fsCreateEntry).toHaveBeenCalledWith(ROOT, "src/lib.rs", false));
    const id = sampleSession.session_id;
    await waitFor(() => expect(useStore.getState().editorTabs[id]).toEqual({ tabs: [["src/lib.rs"]], active: 0 }));
  });

  it("re-lists loaded directories when files change on disk", async () => {
    seedSession();
    render(<FilesPanel />);
    await userEvent.click(await screen.findByText("src")); // load + expand src
    await screen.findByText("main.rs");
    fsListDir.mockClear();

    // A watcher batch touching src re-lists it; an unloaded dir is ignored.
    act(() => useStore.getState().ingestFsChange({ root: ROOT, paths: ["src/lib.rs", "dist/out.js"] }));
    await waitFor(() => expect(fsListDir).toHaveBeenCalledWith(ROOT, "src"));
    expect(fsListDir).not.toHaveBeenCalledWith(ROOT, "dist");

    // Another workspace's changes don't touch this tree.
    fsListDir.mockClear();
    act(() => useStore.getState().ingestFsChange({ root: "/elsewhere", paths: ["README.md"] }));
    expect(fsListDir).not.toHaveBeenCalled();

    // An empty batch means "too much changed": refresh root + expanded dirs.
    act(() => useStore.getState().ingestFsChange({ root: ROOT, paths: [] }));
    await waitFor(() => expect(fsListDir).toHaveBeenCalledWith(ROOT, ""));
    await waitFor(() => expect(fsListDir).toHaveBeenCalledWith(ROOT, "src"));
  });

  it("renders nothing without a workspace", () => {
    const { container } = render(<FilesPanel />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows git changes VS Code-style and opens a diff tab on click", async () => {
    gitStatus.mockResolvedValue([
      { path: "README.md", original_path: null, status: "modified", index: " ", worktree: "M" },
      { path: "src/new.rs", original_path: null, status: "untracked", index: "?", worktree: "?" },
    ]);
    seedSession();
    render(<FilesPanel />);

    // The Changes section lists both, with their status letters.
    const section = await screen.findByRole("region", { name: "Git changes" });
    expect(section).toHaveTextContent("Changes");
    expect(section).toHaveTextContent("2");
    expect(screen.getAllByLabelText("modified").length).toBeGreaterThan(0);

    // The tree row for a changed file carries its badge too.
    const row = await screen.findByTitle("README.md — modified");
    expect(row).toHaveTextContent("M");

    // Clicking a change opens its diff tab (marked path), fronting the editor.
    await userEvent.click(
      screen.getByTitle("README.md — modified · click to view the diff"),
    );
    const id = sampleSession.session_id;
    expect(useStore.getState().editorTabs[id]).toEqual({ tabs: [["diff:README.md"]], active: 0 });
    expect(useStore.getState().rightTab[id]).toBe("editor");

    // The hover affordance opens the file itself, not the diff.
    await userEvent.click(screen.getByRole("button", { name: "Open README.md" }));
    expect(useStore.getState().editorTabs[id]!.tabs).toContainEqual(["README.md"]);
  });

  it("hides the Changes section outside a git repository", async () => {
    gitStatus.mockResolvedValue(null);
    seedSession();
    render(<FilesPanel />);
    await screen.findByText("README.md");
    expect(screen.queryByRole("region", { name: "Git changes" })).not.toBeInTheDocument();
  });
});
