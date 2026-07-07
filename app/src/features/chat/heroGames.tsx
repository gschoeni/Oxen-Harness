import { useEffect, useMemo, useRef, useState } from "react";
import type { ThemePalette } from "../../lib/types";
import type { HeroGameDefinition } from "./games/gameKit";
import { TumbleweedDodgeGame } from "./games/tumbleweed";
import { OxenTrailGame } from "./games/oregonTrail";

export type { HeroGameDefinition } from "./games/gameKit";

type AnyHeroGameDefinition = HeroGameDefinition<any>;

// The empty-state cabinets, in the order the switcher lists them. Add a game by
// implementing HeroGameDefinition (see games/gameKit.tsx) and registering it here.
export const HERO_GAMES = {
  tumbleweed: TumbleweedDodgeGame,
  oregon: OxenTrailGame,
} satisfies Record<string, AnyHeroGameDefinition>;

export type HeroGameName = keyof typeof HERO_GAMES;

export const DEFAULT_HERO_GAME: HeroGameName = "tumbleweed";

export function getHeroGame(name: string | undefined): AnyHeroGameDefinition {
  return HERO_GAMES[(name as HeroGameName) || DEFAULT_HERO_GAME] || TumbleweedDodgeGame;
}

// The Konami-style start combo. Requiring a deliberate sequence keeps stray
// arrow presses (scrolling, editing) from launching the game.
const START_COMBO = ["ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown"];
const ARROW_GLYPHS: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
};
// Keys a game receives while playing, unless its definition widens the set.
const DEFAULT_GAME_KEYS = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " ", "Enter"];

function isEditableTarget(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
}

interface HeroGameProps {
  /** Which registered game to show. */
  gameName: string;
  palette: ThemePalette;
  /** "Press RETURN…"-style bar under the screen (attract only). */
  hint?: string;
  /** When provided, the attract screen shows cabinet-select tabs. */
  onSelectGame?: (name: string) => void;
  variant?: "hero" | "dock";
}

export function HeroGame({ gameName, palette, hint, onSelectGame, variant = "hero" }: HeroGameProps) {
  const definition = useMemo(() => getHeroGame(gameName), [gameName]);
  const [playing, setPlaying] = useState(false);
  const [combo, setCombo] = useState(0);
  const comboRef = useRef(0);
  const [state, setState] = useState(() => definition.initialState());
  const entries = Object.entries(HERO_GAMES) as [string, AnyHeroGameDefinition][];
  const showTabs = !!onSelectGame && entries.length > 1;

  // Switching cabinets resets to that game's attract screen — you re-arm the
  // start combo, which reads as inserting a fresh cartridge.
  useEffect(() => {
    setPlaying(false);
    setCombo(0);
    comboRef.current = 0;
    setState(definition.initialState());
  }, [definition]);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      // Keys aimed at the composer (or any input) never reach the game.
      if (isEditableTarget(e)) return;

      if (!playing) {
        if (!(e.key in ARROW_GLYPHS)) return;
        e.preventDefault();
        const prev = comboRef.current;
        const next = e.key === START_COMBO[prev] ? prev + 1 : e.key === START_COMBO[0] ? 1 : 0;
        if (next >= START_COMBO.length) {
          comboRef.current = 0;
          setCombo(0);
          setState((current: any) => (definition.onStart ? definition.onStart(current) : definition.initialState()));
          setPlaying(true);
        } else {
          comboRef.current = next;
          setCombo(next);
        }
        return;
      }

      if (e.key === "Escape") {
        setPlaying(false);
        return;
      }
      const wants = definition.keys ? definition.keys(e.key) : DEFAULT_GAME_KEYS.includes(e.key);
      if (!wants) return;
      e.preventDefault();
      setState((current: any) => definition.handleKey(current, e.key));
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [definition, playing]);

  // The frame loop only runs while playing; the attract screen is static.
  useEffect(() => {
    if (!playing) return;
    let frame = 0;
    let cancelled = false;
    let last = performance.now();
    const raf = window.requestAnimationFrame || ((cb: FrameRequestCallback) => window.setTimeout(() => cb(performance.now()), 16));
    const caf = window.cancelAnimationFrame || window.clearTimeout;

    function tick(now: number) {
      if (cancelled) return;
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      setState((current: any) => definition.update(current, dt));
      frame = raf(tick);
    }

    frame = raf(tick);
    return () => {
      cancelled = true;
      caf(frame);
    };
  }, [definition, playing]);

  const label = playing
    ? `${definition.title}. Press escape to make camp.`
    : `${definition.title}. Press up, up, down, down to play.`;

  return (
    <div className="hero-game" data-variant={variant} aria-label={label}>
      <div className="hero-game-stage">
        {!playing && definition.renderAttract ? definition.renderAttract(palette) : definition.render(state, palette)}
        {showTabs && !playing && (
          <div className="hero-game-tabs" role="tablist" aria-label="Choose a game">
            {entries.map(([key, def]) => (
              <button
                key={key}
                role="tab"
                aria-selected={key === gameName}
                className={key === gameName ? "hero-game-tab active" : "hero-game-tab"}
                onClick={() => onSelectGame?.(key)}
              >
                {def.tab || def.title}
              </button>
            ))}
          </div>
        )}
        {playing && definition.help && (
          <div className="hero-game-help" aria-hidden="true">
            {definition.help}
          </div>
        )}
      </div>
      {/* The start bar lives below the screen art (not over it): the hint line,
          then the ↑↑↓↓ combo you enter to play. */}
      {!playing && (
        <div className="hero-prompt hero-prompt-start">
          {hint && (
            <span className="hero-prompt-hint">
              {hint}
              <span className="pixel-caret" aria-hidden="true" />
            </span>
          )}
          <span className="hero-game-start" aria-hidden="true">
            <span className="hero-game-combo">
              {START_COMBO.map((key, i) => (
                <kbd key={i} className={i < combo ? "combo-hit" : undefined}>
                  {ARROW_GLYPHS[key]}
                </kbd>
              ))}
            </span>
            <span className="hero-game-start-label">to hit the trail</span>
          </span>
        </div>
      )}
    </div>
  );
}
