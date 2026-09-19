import { beforeEach, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { useWorkbenchAPI } from "./api";
import { useStore } from "../../lib/store";
import { sampleSession } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(resetAll);
it("late view actions add context to their owning conversation", () => {
  useStore.setState({ session: { ...sampleSession, session_id: "a" } });
  const { result } = renderHook(() =>
    useWorkbenchAPI({
      session: "a",
      workspace: "project",
      target: { view: "workflow", path: "a.graph.json" },
    }),
  );
  const api = result.current;
  useStore.setState({ session: { ...sampleSession, session_id: "b" } });
  api.addToChat("Edit this graph");
  expect(useStore.getState().snippets.a?.[0]).toMatchObject({
    path: "a.graph.json",
    code: "Edit this graph",
  });
  expect(useStore.getState().snippets.b).toBeUndefined();
});
