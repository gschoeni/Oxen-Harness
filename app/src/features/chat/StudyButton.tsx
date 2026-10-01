import { GraduationCap } from "lucide-react";
import { Button } from "../../components/ui";
import { openStudyView, STUDY_SHORTCUT, useStudyViewShowing } from "./studyPanel";

/** The composer toolbar's way into the study game. It is always there, at the
 *  far end of the row, so the game is one click away without the chat ever
 *  interrupting to offer it. While the agent works the label makes the pitch
 *  — the wait is the time to learn the code it is changing. It steps aside
 *  once the game is on screen in the work panel. */
export function StudyButton({ busy }: { busy: boolean }) {
  if (useStudyViewShowing()) return null;
  return (
    <Button variant="ghost" size="sm" className="composer-study" onClick={openStudyView} title={`Open the study game (${STUDY_SHORTCUT})`}>
      <GraduationCap size={14} aria-hidden="true" /> {busy ? "Study while it works" : "Study"}
    </Button>
  );
}
