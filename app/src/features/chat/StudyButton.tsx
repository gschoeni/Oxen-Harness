import { GraduationCap } from "lucide-react";
import { Button } from "../../components/ui";
import { useStore } from "../../lib/store";
import { DOCK_SHORTCUT, openStudyDock } from "./studyDock";

/** The composer toolbar's way into the study game. It appears only while the
 *  agent works — the wait is the time to learn the code it is changing, and
 *  the button sits where the eyes already are — and steps aside once the dock
 *  is open. */
export function StudyButton() {
  const open = useStore((s) => s.gameDockOpen);
  if (open) return null;
  return (
    <Button variant="ghost" size="sm" className="composer-study" onClick={openStudyDock} title={`Open the study game (${DOCK_SHORTCUT})`}>
      <GraduationCap size={14} aria-hidden="true" /> Study while it works
    </Button>
  );
}
