import { useMemo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { fsAssetPath, workbenchRequest } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { ViewContext, WorkbenchAPI } from "../../workbench-sdk";

export function useWorkbenchAPI(context: ViewContext): WorkbenchAPI {
  const { session, workspace, target } = context;
  return useMemo(() => {
    const request = <T>(action: string, payload: Record<string, unknown> = {}) =>
      workbenchRequest<T>(session, action, payload);
    return {
      context: { session, workspace, target },
      request,
      read: (path) => request("read", { path }),
      save: (path, content, revision) => request("save", { path, content, revision }),
      subscribeFiles: (handler) =>
        useStore.subscribe((state, previous) => {
          if (state.fsChange !== previous.fsChange && state.fsChange?.root === workspace) handler();
        }),
      asset: async (path) => convertFileSrc(await fsAssetPath(workspace, path)),
      open: (next) => useStore.getState().openWorkView(session, next),
      report: (state) => {
        void request("report", { ...state, view: target.view, path: target.path }).catch((e) =>
          useStore.getState().addNotice(`Report work view: ${String(e)}`),
        );
      },
      addToChat: (text) =>
        useStore
          .getState()
          .addSnippet({ path: target.path ?? "Work view", code: text, start: 1, end: 1 }),
    };
  }, [session, workspace, target]);
}
