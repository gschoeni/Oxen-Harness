// A floating arcade cabinet you can pop open while a turn is streaming, so you
// can play a round without leaving the chat. It renders the same HeroGame the
// empty-state hero does — sharing the store-backed cabinet selection — inside a
// draggable-feeling little window docked to the corner. Games read keyboard
// input globally but ignore it while the composer is focused, so click the dock
// to play and click the composer to type.

import { GripVertical, X } from "lucide-react";
import { useStore } from "../../lib/store";
import { DEFAULT_HERO_GAME, HeroGame } from "./heroGames";
import type { ThemePalette } from "../../lib/types";
import "./gamedock.css";

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

export function GameDock() {
  const palette = useStore((s) => s.theme?.palette) ?? FALLBACK_PALETTE;
  const heroGame = useStore((s) => s.heroGame);
  const themeGame = useStore((s) => (typeof s.theme?.style?.game === "string" ? s.theme.style.game : undefined));
  const setHeroGame = useStore((s) => s.setHeroGame);
  const close = useStore((s) => s.setGameDockOpen);

  // The dock always shows a real game (never the "none" static scene).
  const chosen = heroGame ?? themeGame;
  const gameName = chosen && chosen !== "none" ? chosen : DEFAULT_HERO_GAME;

  return (
    <div className="game-dock" role="dialog" aria-label="Arcade">
      <div className="game-dock-head">
        <GripVertical size={13} className="game-dock-grip" aria-hidden="true" />
        <span className="game-dock-title">Arcade</span>
        <button className="game-dock-close" onClick={() => close(false)} aria-label="Close the arcade">
          <X size={14} />
        </button>
      </div>
      <div className="hero-screen hero-game-screen game-dock-screen">
        <HeroGame gameName={gameName} palette={palette} onSelectGame={setHeroGame} variant="dock" />
      </div>
      <p className="game-dock-foot">Click in to play — your agent keeps working.</p>
    </div>
  );
}
