import { useEffect, useRef } from "react";
import { workbenchCustomizationEnabled } from "../../lib/features";
import { workbenchRequest } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { refreshPackages } from "./packages";
import { useViewRegistry } from "./registry";
import "../../modules";

/** View discovery must work before a file or agent opens the panel. */
export function useWorkbenchRegistration() {
  const registered = useViewRegistry();
  const session = useStore((s) => s.session?.session_id);
  const packagesLoaded = useRef(false);
  const descriptors = JSON.stringify(
    registered
      .filter((view) => view.agentVisible !== false && !view.id.startsWith("package:"))
      .map((view) => ({
        id: view.id,
        title: view.title,
        description: view.description,
        file_patterns: view.filePatterns ?? [],
        requires_file: view.requiresFile ?? false,
        priority: view.priority ?? 0,
        document_schema: view.documentSchema,
      })),
  );
  useEffect(() => {
    if (!session || !workbenchCustomizationEnabled() || packagesLoaded.current) return;
    packagesLoaded.current = true;
    void refreshPackages().catch((e) =>
      useStore.getState().ingestNotice({
        session, kind: "workbench", text: `Load installed views: ${String(e)}`,
      }),
    );
  }, [session]);
  useEffect(() => {
    if (!session) return;
    void workbenchRequest(session, "register_views", {
      views: JSON.parse(descriptors),
    }).catch((e) => useStore.getState().ingestNotice({
      session, kind: "workbench", text: `Register work views: ${String(e)}`,
    }));
  }, [session, descriptors]);
}
