import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ThemePalette } from "../../lib/types";
import { dailyLabel, dailyPreference, pointerAsKey, seedRun, setDailyPreference, type HeroGameDefinition, type PointerInput, type SfxEvent } from "./games/gameKit";
import { playSfx, setSfxPreference, sfxPreference, unlockSfx } from "./games/sfx";
import { TumbleweedDodgeGame } from "./games/tumbleweed";
import { OxenTrailGame } from "./games/oregonTrail";
import { HuntGame } from "./games/hunt";
import "./games/arcade.css";

export type { HeroGameDefinition } from "./games/gameKit";

type AnyHeroGameDefinition = HeroGameDefinition<any>;

// The empty-state cabinets, in the order the switcher lists them. Add a game by
// implementing HeroGameDefinition (see games/gameKit.tsx) and registering it here.
export const HERO_GAMES = {
  tumbleweed: TumbleweedDodgeGame,
  oregon: OxenTrailGame,
  hunt: HuntGame,
} satisfies Record<string, AnyHeroGameDefinition>;

export type HeroGameName = keyof typeof HERO_GAMES;

export const DEFAULT_HERO_GAME: HeroGameName = "tumbleweed";

export function getHeroGame(name: string | undefined): AnyHeroGameDefinition {
  return HERO_GAMES[(name as HeroGameName) || DEFAULT_HERO_GAME] || TumbleweedDodgeGame;
}

// The Konami-style start combo. Requiring a deliberate sequence keeps stray
// arrow presses (scrolling, editing) from launching the game. A click on the
// screen also starts it — for trackpad players who never touch the arrows.
const START_COMBO = ["ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown"];
const ARROW_GLYPHS: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
};
// Keys a game receives while playing, unless its definition widens the set.
const DEFAULT_GAME_KEYS = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " ", "Enter"];
// A pointer that travels less than this (px) is a tap; more is a swipe.
const SWIPE_PX = 18;

function isEditableTarget(e: Event) {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
}

function seedSaltOf(def: AnyHeroGameDefinition, name: string) {
  return def.seedSalt ?? Array.from(name).reduce((h, ch) => (h * 31 + ch.charCodeAt(0)) >>> 0, 7);
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
  const [paused, setPaused] = useState(false);
  const [combo, setCombo] = useState(0);
  const comboRef = useRef(0);
  const [state, setState] = useState(() => definition.initialState());
  // Sound: the hero follows the saved preference; the dock always starts muted
  // so a game popped open mid-turn doesn't chirp over your work.
  const [sound, setSound] = useState(() => (variant === "dock" ? false : sfxPreference()));
  const [daily, setDaily] = useState(() => dailyPreference());
  const lastSfx = useRef(0);
  const stageRef = useRef<HTMLDivElement>(null);
  const pointerStart = useRef<{ x: number; y: number; w: number; h: number } | null>(null);
  const entries = Object.entries(HERO_GAMES) as [string, AnyHeroGameDefinition][];
  const showTabs = !!onSelectGame && entries.length > 1;

  // Switching cabinets resets to that game's attract screen — you re-arm the
  // start combo, which reads as inserting a fresh cartridge.
  useEffect(() => {
    setPlaying(false);
    setPaused(false);
    setCombo(0);
    comboRef.current = 0;
    setState(definition.initialState());
  }, [definition]);

  const start = useCallback(() => {
    comboRef.current = 0;
    setCombo(0);
    unlockSfx();
    seedRun(seedSaltOf(definition, gameName));
    setState((current: any) => (definition.onStart ? definition.onStart(current) : definition.initialState()));
    setPaused(false);
    setPlaying(true);
    stageRef.current?.focus();
    if (sound) playSfx("start");
  }, [definition, gameName, sound]);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      // Keys aimed at the composer (or any input) never reach the game.
      if (isEditableTarget(e) || e.repeat || (e.target as HTMLElement | null)?.closest("button")) return;

      if (!playing) {
        if (!(e.key in ARROW_GLYPHS)) {
          // Non-arrow keys on the attract screen go to the game's pre-play
          // choices (rider, difficulty…), if it offers any.
          if (definition.handleAttractKey && e.key.length === 1) {
            setState((current: any) => definition.handleAttractKey!(current, e.key));
          }
          return;
        }
        e.preventDefault();
        const prev = comboRef.current;
        const next = e.key === START_COMBO[prev] ? prev + 1 : e.key === START_COMBO[0] ? 1 : 0;
        if (next >= START_COMBO.length) {
          start();
        } else {
          comboRef.current = next;
          setCombo(next);
        }
        return;
      }

      if (e.key === "Escape") {
        setPlaying(false);
        setPaused(false);
        return;
      }
      if (e.key.toLowerCase() === "p") {
        e.preventDefault();
        setPaused((value) => !value);
        if (definition.onPause) setState((current: any) => definition.onPause!(current));
        return;
      }
      // Any key resumes from a pause; the key itself is swallowed so a resume
      // never doubles as a move.
      if (paused) {
        e.preventDefault();
        setPaused(false);
        return;
      }
      const wants = definition.keys ? definition.keys(e.key) : DEFAULT_GAME_KEYS.includes(e.key);
      if (!wants) return;
      e.preventDefault();
      unlockSfx();
      setState((current: any) => definition.handleKey(current, e.key));
    }

    // Releases only matter to games with hold-to-move controls.
    function onKeyUp(e: KeyboardEvent) {
      if (!playing || !definition.handleKeyUp || isEditableTarget(e)) return;
      const wants = definition.keys ? definition.keys(e.key) : DEFAULT_GAME_KEYS.includes(e.key);
      if (!wants) return;
      setState((current: any) => definition.handleKeyUp!(current, e.key));
    }

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, [definition, playing, paused, start]);

  // Losing the window mid-run pauses instead of letting the ox crash while you
  // answer a message; any key or click brings it back.
  useEffect(() => {
    if (!playing) return;
    function pause() {
      setPaused(true);
      if (definition.onPause) setState((current: any) => definition.onPause!(current));
    }
    function onVisibility() {
      if (document.hidden) pause();
    }
    function onFocus(e: FocusEvent) {
      if (isEditableTarget(e)) pause();
    }
    document.addEventListener("focusin", onFocus);
    window.addEventListener("blur", pause);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("focusin", onFocus);
      window.removeEventListener("blur", pause);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [definition, playing]);

  // The frame loop only runs while playing and unpaused; the attract screen is static.
  useEffect(() => {
    if (!playing || paused) return;
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
  }, [definition, playing, paused]);

  // Play whatever cues the game queued since the last render.
  useEffect(() => {
    const queue: SfxEvent[] | undefined = (state as any)?.sfx;
    if (!queue || queue.length === 0) return;
    const newest = queue[queue.length - 1].id;
    if (newest <= lastSfx.current) return;
    if (sound && playing) {
      for (const cue of queue) if (cue.id > lastSfx.current) playSfx(cue.name);
    }
    lastSfx.current = newest;
  }, [state, sound, playing]);

  function toggleSound() {
    const next = !sound;
    setSound(next);
    if (variant !== "dock") setSfxPreference(next);
    if (next) {
      unlockSfx();
      playSfx("menu");
    }
  }

  function toggleDaily() {
    const next = !daily;
    setDaily(next);
    setDailyPreference(next);
  }

  // Pointer play: a click on the attract screen starts; while playing, taps and
  // swipes on the field reach the game (as arrow keys unless it maps them).
  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    if ((e.target as HTMLElement).closest("button")) return;
    const rect = e.currentTarget.getBoundingClientRect();
    pointerStart.current = { x: e.clientX - rect.left, y: e.clientY - rect.top, w: rect.width, h: rect.height };
  }

  function onPointerUp(e: React.PointerEvent<HTMLDivElement>) {
    const s0 = pointerStart.current;
    pointerStart.current = null;
    if (!s0 || (e.target as HTMLElement).closest("button")) return;
    stageRef.current?.focus();
    const rect = e.currentTarget.getBoundingClientRect();
    const dx = e.clientX - rect.left - s0.x;
    const dy = e.clientY - rect.top - s0.y;
    unlockSfx();
    if (!playing) {
      start();
      return;
    }
    if (paused) {
      setPaused(false);
      return;
    }
    let input: PointerInput;
    if (Math.hypot(dx, dy) < SWIPE_PX) {
      input = { kind: "tap", x: s0.x / s0.w, y: s0.y / s0.h };
    } else {
      const dir = Math.abs(dx) > Math.abs(dy) ? (dx < 0 ? "ArrowLeft" : "ArrowRight") : dy < 0 ? "ArrowUp" : "ArrowDown";
      input = { kind: "swipe", x: s0.x / s0.w, y: s0.y / s0.h, dir };
    }
    setState((current: any) => (definition.handlePointer ? definition.handlePointer(current, input) : definition.handleKey(current, pointerAsKey(input))));
  }

  const label = playing
    ? paused
      ? `${definition.title}. Paused — press any key to resume.`
      : `${definition.title}. Press escape to make camp.`
    : `${definition.title}. Press up, up, down, down to play, or click the screen.`;

  return (
    <div className="hero-game" data-variant={variant} aria-label={label}>
      <div className="arcade-toolbar">
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
        {/* Cabinet switches: sound and the daily seed. Small, corner-mounted,
            available on the attract screen and while playing. */}
        <div className="hero-game-switches">
          {!playing && (
            <button className={daily ? "hero-game-switch active" : "hero-game-switch"} onClick={toggleDaily} aria-pressed={daily} title="Daily run: everyone gets today's trail">
              {daily ? `DAILY ${dailyLabel()}` : "DAILY"}
            </button>
          )}
          <button className={sound ? "hero-game-switch active" : "hero-game-switch"} onClick={toggleSound} aria-pressed={sound} aria-label={sound ? "Mute game sound" : "Unmute game sound"} title={sound ? "Sound on" : "Sound off"}>
            {sound ? "♪ ON" : "♪ OFF"}
          </button>
        </div>
          {playing && <>
            <button className="hero-game-switch" aria-label={paused ? "Resume game" : "Pause game"} onClick={() => {
              setPaused(!paused);
              if (definition.onPause) setState((current: any) => definition.onPause!(current));
              stageRef.current?.focus();
            }}>{paused ? "RESUME" : "PAUSE"}</button>
            <button className="hero-game-switch" aria-label="Back to arcade menu" onClick={() => { setPlaying(false); setPaused(false); }}>MENU</button>
          </>}
      </div>
      <div ref={stageRef} tabIndex={0} className="hero-game-stage" aria-label={`${definition.title} playfield`} onPointerDown={onPointerDown} onPointerUp={onPointerUp} onPointerCancel={() => (pointerStart.current = null)}>
        {!playing && definition.renderAttract ? definition.renderAttract(palette) : definition.render(state, palette)}
        {playing && paused && (
          <div className="hero-game-pause" role="status">
            <span>PAUSED</span>
            <small>press any key</small>
          </div>
        )}
      </div>
      {playing && definition.help && <div className="arcade-controls">{definition.help}</div>}
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
          <button className="hero-game-start arcade-play" onClick={start} aria-label={`Play ${definition.title}`}>
            <span className="hero-game-combo">
              {START_COMBO.map((key, i) => (
                <kbd key={i} className={i < combo ? "combo-hit" : undefined}>
                  {ARROW_GLYPHS[key]}
                </kbd>
              ))}
            </span>
            <span className="hero-game-start-label">PLAY <span aria-hidden="true">↵</span></span>
          </button>
        </div>
      )}
    </div>
  );
}
