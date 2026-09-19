import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../preview/useOverlayOpen", () => ({ useOverlayOpen: () => false }));
import { viewPackageMount, viewPackageClose } from "../../test/ipcMock";
import { PackageSurface } from "./PackageSurface";
import type { WorkbenchAPI } from "../../workbench-sdk";
beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  viewPackageMount
    .mockClear()
    .mockResolvedValueOnce("first")
    .mockResolvedValueOnce("second");
  viewPackageClose.mockClear();
});
afterEach(() => vi.unstubAllGlobals());
it("retains the native surface during status polls but replaces it for a new preview generation", async () => {
  const api = {
    context: {
      session: "s",
      workspace: "project",
      target: { view: "view-studio", path: "views/demo" },
    },
    report: vi.fn(),
  } as unknown as WorkbenchAPI;
  const { rerender, unmount } = render(
    <PackageSurface api={api} packageId="demo" development revision="hash:1" />,
  );
  await waitFor(() => expect(viewPackageMount).toHaveBeenCalledOnce());
  rerender(
    <PackageSurface
      api={{ ...api }}
      packageId="demo"
      development
      revision="hash:1"
    />,
  );
  expect(viewPackageMount).toHaveBeenCalledOnce();
  rerender(
    <PackageSurface
      api={{ ...api }}
      packageId="demo"
      development
      revision="hash:2"
    />,
  );
  await waitFor(() => expect(viewPackageMount).toHaveBeenCalledTimes(2));
  expect(viewPackageClose).toHaveBeenCalledWith("first");
  unmount();
  expect(viewPackageClose).toHaveBeenCalledWith("second");
});
