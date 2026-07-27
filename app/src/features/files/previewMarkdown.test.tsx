import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { MarkdownPreview, resolveWorkspaceUrl } from "./previewMarkdown";
import { sampleSession } from "../../test/ipcMock";
import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";

describe("resolveWorkspaceUrl", () => {
  it("resolves relative, ./-, and root-relative paths against the right base", () => {
    expect(resolveWorkspaceUrl("docs", "setup.md")).toEqual({ rel: "docs/setup.md", fragment: "" });
    expect(resolveWorkspaceUrl("docs", "./setup.md")).toEqual({ rel: "docs/setup.md", fragment: "" });
    expect(resolveWorkspaceUrl("docs", "../README.md")).toEqual({ rel: "README.md", fragment: "" });
    // Root-relative resolves against the workspace, GitHub-style.
    expect(resolveWorkspaceUrl("docs", "/images/logo.svg")).toEqual({
      rel: "images/logo.svg",
      fragment: "",
    });
    expect(resolveWorkspaceUrl("", "CONTRIBUTING.md")?.rel).toBe("CONTRIBUTING.md");
  });

  it("keeps fragments and drops query strings", () => {
    expect(resolveWorkspaceUrl("", "/images/logo.svg#gh-dark-mode-only")).toEqual({
      rel: "images/logo.svg",
      fragment: "gh-dark-mode-only",
    });
    expect(resolveWorkspaceUrl("", "page.md?plain=1")?.rel).toBe("page.md");
  });

  it("refuses externals, fragments, and workspace escapes", () => {
    expect(resolveWorkspaceUrl("", "https://oxen.ai/")).toBeNull();
    expect(resolveWorkspaceUrl("", "mailto:hi@oxen.ai")).toBeNull();
    expect(resolveWorkspaceUrl("", "//cdn.example.com/x.png")).toBeNull();
    expect(resolveWorkspaceUrl("", "#usage")).toBeNull();
    expect(resolveWorkspaceUrl("", "../outside.md")).toBeNull();
    expect(resolveWorkspaceUrl("docs", "../../outside.md")).toBeNull();
  });
});

describe("MarkdownPreview", () => {
  beforeEach(() => {
    resetAll();
    useStore.setState({ session: sampleSession, mode: "dark" });
  });

  const WS = sampleSession.workspace;

  it("renders embedded HTML as real links, sanitized", async () => {
    render(
      <MarkdownPreview
        workspace={WS}
        path="README.md"
        content={`<div align="center"><a href="https://docs.oxen.ai/">Docs</a></div>\n<script>window.pwned = true</script>`}
      />,
    );
    const link = await screen.findByRole("link", { name: "Docs" });
    expect(link).toHaveAttribute("href", "https://docs.oxen.ai/");
    // Sanitizer stripped the script.
    expect(document.querySelector("script")).toBeNull();
    expect((window as { pwned?: boolean }).pwned).toBeUndefined();
  });

  it("resolves local images through the asset protocol and follows gh-mode variants", async () => {
    render(
      <MarkdownPreview
        workspace={WS}
        path="README.md"
        content={[
          "![dark logo](/images/dark.svg#gh-dark-mode-only)",
          "![light logo](/images/light.svg#gh-light-mode-only)",
          "![remote](https://img.shields.io/badge/x.svg)",
        ].join("\n\n")}
      />,
    );
    // Dark mode: the dark variant renders, resolved into the workspace...
    const dark = await screen.findByAltText("dark logo");
    expect(dark.getAttribute("src")).toContain("images/dark.svg");
    expect(dark.getAttribute("src")).not.toBe("/images/dark.svg");
    // ...the light variant is skipped entirely, and remote srcs pass through.
    expect(screen.queryByAltText("light logo")).toBeNull();
    expect(screen.getByAltText("remote")).toHaveAttribute(
      "src",
      "https://img.shields.io/badge/x.svg",
    );
  });

  it("opens repo-relative links in the editor pane instead of the browser", async () => {
    render(
      <MarkdownPreview
        workspace={WS}
        path="docs/index.md"
        content={"[setup](./setup.md)"}
      />,
    );
    const link = await screen.findByRole("link", { name: "setup" });
    // No href: the global click interceptor must not ship it to the browser.
    expect(link).not.toHaveAttribute("href");
    await userEvent.click(link);
    const id = sampleSession.session_id;
    expect(useStore.getState().editorTabs[id]).toEqual({
      tabs: [["docs/setup.md"]],
      active: 0,
    });
  });
});
