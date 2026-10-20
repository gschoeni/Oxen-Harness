// The title bar's "update available" pill: shown only when a newer release
// is out, and one click from Settings → Updates.

import { ArrowUpCircle } from "lucide-react";
import { useStore } from "../../lib/store";
import { useUpdate, useUpdateCheck } from "./update";
import "./update.css";

export function UpdatePill() {
  useUpdateCheck();
  const latest = useUpdate((s) => (s.status?.available ? s.status.latest : null));
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const setSettingsPage = useStore((s) => s.setSettingsPage);
  if (!latest) return null;
  return (
    <button
      className="update-pill"
      title={`Version ${latest} is available — open Updates`}
      onClick={() => {
        setSettingsPage("updates");
        setSettingsOpen(true);
      }}
    >
      <ArrowUpCircle size={14} strokeWidth={1.75} aria-hidden="true" />
      <span>Update to v{latest}</span>
    </button>
  );
}
