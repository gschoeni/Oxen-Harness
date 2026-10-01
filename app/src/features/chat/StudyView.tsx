// The game as a work view: the right-hand panel hosts the same cabinet the
// empty-state hero does, so you can play while a turn streams without anything
// floating over the chat. The study cabinet starts itself with the keyboard;
// the arcade cabinets (behind MENU) wait for a click.

import { useStore } from "../../lib/store";
import { DEFAULT_HERO_GAME, HeroGame } from "./heroGames";
import { useGameHost } from "./games/studyHost";
import { STUDY_SHORTCUT } from "./studyPanel";
import type { ThemePalette } from "../../lib/types";
import "./study.css";

// Used only if a theme somehow has no palette; the games also self-default per
// field, so this is belt-and-suspenders.
const FALLBACK_PALETTE: ThemePalette = {
  title: "#f0be8c",
  primary: "#60b060",
  secondary: "#aa6e3c",
  text: "#ece2ce",
  muted: "#968d7d",
  danger: "#c94c4c",
  link: "#f0be8c",
  background: "#0f1115",
  surface: "#17191f",
  border: "#2a2d35",
};

export function StudyView() {
  const palette = useStore((s) => s.theme?.palette) ?? FALLBACK_PALETTE;
  const heroGame = useStore((s) => s.heroGame);
  const themeGame = useStore((s) => (typeof s.theme?.style?.game === "string" ? s.theme.style.game : undefined));
  const setHeroGame = useStore((s) => s.setHeroGame);
  // Before a chat's first message the hero already shows this cabinet in the
  // chat column; two live cabinets would both take the same key presses.
  const started = useStore((s) => !!s.session && (s.threads[s.session.session_id] ?? []).some((it) => it.kind !== "notice"));
  const gameHost = useGameHost();

  // The panel always shows a real game (never the "none" static scene).
  const chosen = heroGame ?? themeGame;
  const gameName = chosen && chosen !== "none" ? chosen : DEFAULT_HERO_GAME;
  const studying = gameName === "study";

  if (!started) {
    return (
      <div className="workbench-welcome">
        <p>The game is on the main screen until this chat gets going. Once your agent is working, it plays here.</p>
      </div>
    );
  }
  return (
    <div className="study-view">
      <div className="hero-screen hero-game-screen study-view-screen">
        <HeroGame gameName={gameName} palette={palette} onSelectGame={setHeroGame} variant="panel" host={gameHost} autoStart={studying} />
      </div>
      <p className="study-view-foot">
        {studying
          ? `Press 1-4 to pick a trail — your agent keeps working. ${STUDY_SHORTCUT} hides this.`
          : `Click in to play — your agent keeps working. ${STUDY_SHORTCUT} hides this.`}
      </p>
    </div>
  );
}
