import { useEffect } from "react";
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
    if (!workbenchCustomizationEnabled()) return;
    void refreshPackages().catch((e) =>
      useStore.getState().addNotice(`Load installed views: ${String(e)}`),
    );
  }, []);
  useEffect(() => {
    if (!session) return;
    void workbenchRequest(session, "register_views", {
      views: JSON.parse(descriptors),
    }).catch((e) => useStore.getState().addNotice(`Register work views: ${String(e)}`));
  }, [session, descriptors]);
}
