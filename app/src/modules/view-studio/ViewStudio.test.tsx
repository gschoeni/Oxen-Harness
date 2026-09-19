import { expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ViewStudio, type StudioStatus } from "./ViewStudio";
import type { WorkbenchAPI } from "../../workbench-sdk";
vi.mock("../../features/workbench/packages", () => ({
  refreshPackages: vi.fn(async () => []),
}));
vi.mock("../../features/workbench/PackageSurface", () => ({
  PackageSurface: ({ resourcePath }: { resourcePath?: string }) => (
    <div>
      Live package<span data-testid="resource">{resourcePath}</span>
    </div>
  ),
}));
const candidate = {
  digest: "revision-one",
  manifest: {
    id: "my.view",
    title: "My view",
    description: "",
    api_version: 1,
    entry: "index.html",
    file_patterns: [],
    permissions: {
      read: ["data/my.view/*.json"],
      write: ["data/my.view/*.json"],
      assets: [],
      actions: [],
    },
  },
};
const checked: StudioStatus = {
  source: "views/my-view",
  report_path: ".oxen-harness/view-dev/test.json",
  active: false,
  paused: false,
  dirty: false,
  mounted: false,
  candidate,
  diagnostics: [],
  runtime: [],
};
it("scaffolds, reviews the declared grants, and previews exactly the checked revision", async () => {
  const request = vi.fn(async (_action, payload) =>
    payload.action === "preview"
      ? { ...checked, active: true, package: candidate }
      : checked,
  );
  const api = {
    context: {
      session: "s",
      workspace: "project",
      target: { view: "view-studio" },
    },
    request,
    open: vi.fn(),
    addToChat: vi.fn(),
  } as unknown as WorkbenchAPI;
  render(<ViewStudio api={api} />);
  await userEvent.click(screen.getByRole("button", { name: "Create starter" }));
  expect(request).toHaveBeenCalledWith("develop", {
    action: "scaffold",
    source: "views/my-view",
    id: "my.view",
    title: "My view",
  });
  expect(screen.getByText("read")).toBeInTheDocument();
  await userEvent.click(
    screen.getByRole("button", { name: "Start live preview" }),
  );
  expect(request).toHaveBeenLastCalledWith("develop", {
    action: "preview",
    source: "views/my-view",
    digest: "revision-one",
  });
  expect(await screen.findByText("Live package")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Test" })).toBeDisabled();
});
it("shows scaffold errors without silently replacing a package", async () => {
  const api = {
    context: {
      session: "s",
      workspace: "project",
      target: { view: "view-studio" },
    },
    request: vi.fn().mockRejectedValue(new Error("already exists")),
    open: vi.fn(),
    addToChat: vi.fn(),
  } as unknown as WorkbenchAPI;
  render(<ViewStudio api={api} />);
  await userEvent.click(screen.getByRole("button", { name: "Create starter" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("already exists");
  expect(api.open).not.toHaveBeenCalled();
});
it("changes the preview document only after applying it and protects unretained drafts", async () => {
  const status = {
    ...checked,
    active: true,
    mounted: true,
    package: candidate,
    previous: candidate,
  };
  const api = {
    context: {
      session: "s",
      workspace: "project",
      target: { view: "view-studio", path: "views/my-view" },
    },
    request: vi.fn().mockResolvedValue(status),
    open: vi.fn(),
    addToChat: vi.fn(),
  } as unknown as WorkbenchAPI;
  const { rerender } = render(<ViewStudio api={api} />);
  await userEvent.click(
    await screen.findByRole("button", { name: "Checks & console" }),
  );
  await userEvent.type(
    screen.getByLabelText("Preview document (optional)"),
    "data/my.view/other.json",
  );
  expect(screen.getByTestId("resource")).toBeEmptyDOMElement();
  await userEvent.click(screen.getByRole("button", { name: "Apply document" }));
  expect(screen.getByTestId("resource")).toHaveTextContent(
    "data/my.view/other.json",
  );
  const dirtyApi = {
    ...api,
    request: vi.fn().mockResolvedValue({ ...status, dirty: true }),
  } as unknown as WorkbenchAPI;
  rerender(<ViewStudio api={dirtyApi} />);
  expect(
    await screen.findByRole("button", { name: "Reload latest preview" }),
  ).toBeDisabled();
  expect(
    screen.getByRole("button", { name: /Previous preview/ }),
  ).toBeDisabled();
  expect(screen.getByRole("button", { name: "Apply document" })).toBeDisabled();
});
