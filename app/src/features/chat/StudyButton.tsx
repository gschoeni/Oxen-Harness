import { GraduationCap } from "lucide-react";
import { Button } from "../../components/ui";
import { openStudyView, STUDY_SHORTCUT, useStudyViewShowing } from "./studyPanel";

/** The composer toolbar's way into the study game. It appears only while the
 *  agent works — the wait is the time to learn the code it is changing, and
 *  the button sits where the eyes already are — and steps aside once the game
 *  is on screen in the work panel. */
export function StudyButton() {
  if (useStudyViewShowing()) return null;
  return (
    <Button variant="ghost" size="sm" className="composer-study" onClick={openStudyView} title={`Open the study game (${STUDY_SHORTCUT})`}>
      <GraduationCap size={14} aria-hidden="true" /> Study while it works
    </Button>
  );
}
