// A quiet offer to open the study cabinet. The game only helps if it gets
// played, so when a long turn ends the chat offers a ride-along on what the
// agent just changed, before you move on. While the turn is still running the
// composer's Study button is the only invitation — a second one above it was
// noise.
//
// It never interrupts: one line above the composer, gone when the next turn
// starts, and "×" silences it until the app restarts.

import { GraduationCap, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { openStudyView, useStudyViewShowing } from "./studyPanel";
import "./study.css";

/** How long a turn has to run to earn the offer when it ends. */
export const NUDGE_AFTER_MS = 45_000;
/** How long the after-turn offer stays up. */
export const NUDGE_LINGER_MS = 60_000;

// Module-level so a dismissal outlives the component (it remounts per chat).
let silenced = false;

/** Test hook: forget a dismissal. */
export function resetStudyNudge() {
  silenced = false;
}

export function StudyNudge({ running }: { running: boolean }) {
  const showing = useStudyViewShowing();
  const [offered, setOffered] = useState(false);
  const ranLong = useRef(false);

  useEffect(() => {
    if (running) {
      ranLong.current = false;
      setOffered(false);
      const timer = window.setTimeout(() => {
        ranLong.current = true;
      }, NUDGE_AFTER_MS);
      return () => window.clearTimeout(timer);
    }
    // The turn ended. Only a long one earns the offer; a quick reply isn't
    // worth a quiz.
    if (!ranLong.current) {
      setOffered(false);
      return;
    }
    ranLong.current = false;
    setOffered(true);
    const timer = window.setTimeout(() => setOffered(false), NUDGE_LINGER_MS);
    return () => window.clearTimeout(timer);
  }, [running]);

  if (!offered || showing || silenced) return null;

  function open() {
    openStudyView();
    setOffered(false);
  }

  function dismiss() {
    silenced = true;
    setOffered(false);
  }

  return (
    <div className="study-nudge" role="status">
      <GraduationCap size={14} aria-hidden="true" />
      <span className="study-nudge-text">
        The agent's done. Can you explain what it changed? Try a Ride-along.
      </span>
      <button className="study-nudge-go" onClick={open}>
        Study
      </button>
      <button className="study-nudge-x" onClick={dismiss} aria-label="Stop offering the study game">
        <X size={13} />
      </button>
    </div>
  );
}
