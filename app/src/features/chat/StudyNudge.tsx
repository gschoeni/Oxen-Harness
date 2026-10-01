// A quiet offer to open the study cabinet. The game only helps if it gets
// played, and nobody remembers the gamepad icon mid-task — so when a turn has
// run for a while the chat says so once, and when a long turn ends it offers
// a ride-along on what the agent just changed, before you move on.
//
// It never interrupts: one line above the composer, gone when the turn's
// state changes, and "×" silences it until the app restarts.

import { GraduationCap, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useStore } from "../../lib/store";
import "./gamedock.css";

/** How long a turn runs before the offer appears. */
export const NUDGE_AFTER_MS = 45_000;
/** How long the after-turn offer stays up. */
export const NUDGE_LINGER_MS = 60_000;

type Offer = "none" | "during" | "after";

// Module-level so a dismissal outlives the component (it remounts per chat).
let silenced = false;

/** Test hook: forget a dismissal. */
export function resetStudyNudge() {
  silenced = false;
}

export function StudyNudge({ running }: { running: boolean }) {
  const dockOpen = useStore((s) => s.gameDockOpen);
  const setHeroGame = useStore((s) => s.setHeroGame);
  const setGameDockOpen = useStore((s) => s.setGameDockOpen);
  const [offer, setOffer] = useState<Offer>("none");
  const ranLong = useRef(false);

  useEffect(() => {
    if (running) {
      ranLong.current = false;
      setOffer("none");
      const timer = window.setTimeout(() => {
        ranLong.current = true;
        setOffer("during");
      }, NUDGE_AFTER_MS);
      return () => window.clearTimeout(timer);
    }
    // The turn ended. Only a long one earns the follow-up; a quick reply
    // isn't worth a quiz.
    if (!ranLong.current) {
      setOffer("none");
      return;
    }
    ranLong.current = false;
    setOffer("after");
    const timer = window.setTimeout(() => setOffer("none"), NUDGE_LINGER_MS);
    return () => window.clearTimeout(timer);
  }, [running]);

  if (offer === "none" || dockOpen || silenced) return null;

  function open() {
    setHeroGame("study");
    setGameDockOpen(true);
    setOffer("none");
  }

  function dismiss() {
    silenced = true;
    setOffer("none");
  }

  return (
    <div className="study-nudge" role="status">
      <GraduationCap size={14} aria-hidden="true" />
      <span className="study-nudge-text">
        {offer === "during"
          ? "Your agent's been at it a while. Learn the code it's working in?"
          : "The agent's done. Can you explain what it changed? Try a Ride-along."}
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
