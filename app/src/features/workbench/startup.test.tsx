import { expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("../../lib/uiState", async (original) => ({
  ...await original<typeof import("../../lib/uiState")>(),
  getUi: (key: string) => ({
    workContexts: {
      s1: { current: { view: "editor", path: "saved.txt" }, history: [{ view: "editor", path: "saved.txt" }], cursor: 0, pinned: false },
    },
    docks: { widths: { right: 500 }, collapsed: { right: false } },
  })[key as "workContexts" | "docks"],
}));

import App from "../../App";
import { useStore } from "../../lib/store";
import { sampleSession, sessionInfo, workbenchRequest } from "../../test/ipcMock";

it("restores history without opening the panel and registers views for the agent", async () => {
  sessionInfo.mockResolvedValueOnce({ ...sampleSession, session_id: "s1" });
  const { container } = render(<App />);
  await waitFor(() => expect(useStore.getState().session?.session_id).toBe("s1"));
  expect(useStore.getState().workContexts.s1.current.path).toBe("saved.txt");
  expect(container.querySelector(".workbench")).not.toBeInTheDocument();
  expect(container.querySelector(".dock-rail.right")).not.toBeInTheDocument();
  expect(container.querySelector(".app")).toHaveStyle({ "--dock-right-w": "0px" });
  await waitFor(() => expect(workbenchRequest).toHaveBeenCalledWith(
    "s1", "register_views", expect.objectContaining({ views: expect.arrayContaining([expect.objectContaining({ id: "editor" })]) }),
  ));
  // The workflow view is behind a release flag, so the agent is never told it exists.
  for (const [, action, payload] of workbenchRequest.mock.calls) {
    if (action === "register_views") expect(payload).not.toEqual(
      expect.objectContaining({ views: expect.arrayContaining([expect.objectContaining({ id: "workflow" })]) }),
    );
  }
});
