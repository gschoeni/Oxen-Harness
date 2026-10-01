import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ThemePalette } from "../../lib/types";
import { dailyLabel, dailyPreference, pointerAsKey, seedRun, setDailyPreference, type GameRequest, type GameResult, type HeroGameDefinition, type HeroGameHost, type PointerInput, type SfxEvent } from "./games/gameKit";
import { playSfx, setSfxPreference, sfxPreference, unlockSfx } from "./games/sfx";
import { TumbleweedDodgeGame } from "./games/tumbleweed";
import { OxenTrailGame } from "./games/oregonTrail";
import { HuntGame } from "./games/hunt";
import { StudyGame } from "./games/study";
import "./games/arcade.css";

export type { HeroGameDefinition, HeroGameHost } from "./games/gameKit";

type AnyHeroGameDefinition = HeroGameDefinition<any>;

// The empty-state cabinets, in the order the switcher lists them. Add a game by
// implementing HeroGameDefinition (see games/gameKit.tsx) and registering it here.
export const HERO_GAMES = {
  tumbleweed: TumbleweedDodgeGame,
  oregon: OxenTrailGame,
  hunt: HuntGame,
  study: StudyGame,
} satisfies Record<string, AnyHeroGameDefinition>;

export type HeroGameName = keyof typeof HERO_GAMES;

// What a player who has never picked a cabinet sees (a saved choice, then a
// theme's `[style] game`, come first). Study leads because it is the one game
// about the user's own project; its attract screen needs no model or key.
export const DEFAULT_HERO_GAME: HeroGameName = "study";

export function getHeroGame(name: string | undefined): AnyHeroGameDefinition {
  return HERO_GAMES[(name as HeroGameName) || DEFAULT_HERO_GAME] || HERO_GAMES[DEFAULT_HERO_GAME];
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
  /** The backend for cabinets that queue requests (see gameKit's
      `HeroGameHost`). Without one, a request fails with a clear message. */
  host?: HeroGameHost;
  /** Skip the attract screen and take the keyboard as soon as this mounts —
      for a cabinet opened on purpose (the dock's study game), where a start
      combo is one more step between "I'll study" and the first question. */
  autoStart?: boolean;
}

export function HeroGame({ gameName, palette, hint, onSelectGame, variant = "hero", host, autoStart = false }: HeroGameProps) {
  const definition = useMemo(() => getHeroGame(gameName), [gameName]);
  const definitionRef = useRef(definition);
  definitionRef.current = definition;
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
  const lastRequest = useRef(0);
  const [entryText, setEntryText] = useState("");
  const entryRef = useRef<HTMLInputElement>(null);
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

  const changePause = useCallback((next: boolean) => {
    setPaused(next);
    if (definition.onPause) setState((current: any) => definition.onPause!(current));
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

  // Declared after the cabinet-reset effect above, so on mount (and on a swap
  // to an auto-starting cabinet) the reset runs first and this start sticks.
  const autoStartedFor = useRef<typeof definition | null>(null);
  useEffect(() => {
    if (!autoStart || autoStartedFor.current === definition) return;
    autoStartedFor.current = definition;
    start();
  }, [autoStart, definition, start]);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const cabinet = document.activeElement?.closest(".hero-game") ?? (e.target as HTMLElement | null)?.closest(".hero-game");
      if (cabinet && cabinet !== stageRef.current?.parentElement) return;
      // Keys aimed at the composer (or any input) never reach the game, and
      // neither do app shortcuts: ⌘1 switches a tab, it doesn't answer "1".
      if (isEditableTarget(e) || e.repeat || e.metaKey || e.ctrlKey || e.altKey || (e.target as HTMLElement | null)?.closest("button")) return;

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
        changePause(!paused);
        return;
      }
      // Any key resumes from a pause; the key itself is swallowed so a resume
      // never doubles as a move.
      if (paused) {
        if (["Tab", "Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
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
  }, [definition, playing, paused, start, changePause]);

  // Losing the window mid-run pauses instead of letting the ox crash while you
  // answer a message; any key or click brings it back.
  useEffect(() => {
    if (!playing) return;
    function pause() {
      changePause(true);
    }
    function onVisibility() {
      if (document.hidden) pause();
    }
    function onFocus(e: FocusEvent) {
      const cabinet = (e.target as HTMLElement | null)?.closest(".hero-game");
      // The cabinet's own answer box is part of play, not a reason to pause.
      if (cabinet && cabinet === stageRef.current?.parentElement) return;
      if (isEditableTarget(e) || cabinet) pause();
    }
    document.addEventListener("focusin", onFocus);
    window.addEventListener("blur", pause);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("focusin", onFocus);
      window.removeEventListener("blur", pause);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [playing, changePause]);

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

  // Perform whatever the game asked of the backend since the last render, and
  // hand each outcome back through `deliver`. A reply that lands after the
  // cabinet was swapped is dropped: it belongs to a state that no longer exists.
  useEffect(() => {
    const queue: GameRequest[] | undefined = (state as any)?.requests;
    if (!queue || queue.length === 0 || !definition.deliver) return;
    const fresh = queue.filter((request) => request.id > lastRequest.current);
    if (fresh.length === 0) return;
    lastRequest.current = queue[queue.length - 1].id;
    for (const request of fresh) {
      const settle = (result: GameResult) => {
        if (definitionRef.current !== definition) return;
        setState((current: any) => definition.deliver!(current, request, result));
      };
      if (!host) {
        settle({ ok: false, error: "This game needs the app's backend, which isn't connected here." });
        continue;
      }
      host.perform(request.kind, request.payload).then(
        (value) => settle({ ok: true, value }),
        (error) => settle({ ok: false, error: String(error?.message ?? error) }),
      );
    }
  }, [state, definition, host]);

  // A game waiting on typed input gets a real text box under the screen.
  const entry = playing && !paused && definition.textEntry ? definition.textEntry(state) : null;
  const entryKey = entry?.key ?? null;
  const lastEntryKey = useRef<string | null>(null);
  useEffect(() => {
    if (entryKey === null) return;
    // A pause unmounts the box; the draft only goes when the question does.
    if (entryKey !== lastEntryKey.current) {
      lastEntryKey.current = entryKey;
      setEntryText("");
    }
    entryRef.current?.focus();
  }, [entryKey]);

  function submitEntry(e: React.FormEvent) {
    e.preventDefault();
    if (!definition.handleText) return;
    const text = entryText;
    setEntryText("");
    setState((current: any) => definition.handleText!(current, text));
  }

  function toggleSound() {
    const next = !sound;
    setSound(next);
    if (variant !== "dock") setSfxPreference(next);
    if (playing) stageRef.current?.focus();
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
    <div className="hero-game" data-variant={variant} data-game={gameName} aria-label={label}>
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
        {/* These controls must never cover the game HUD. */}
        <div className="hero-game-switches">
          {!playing && !definition.noDaily && (
            <button className={daily ? "hero-game-switch active" : "hero-game-switch"} onClick={toggleDaily} aria-pressed={daily} title="Daily run: everyone gets today's trail">
              {daily ? `DAILY ${dailyLabel()}` : "DAILY"}
            </button>
          )}
          <button className={sound ? "hero-game-switch active" : "hero-game-switch"} onClick={toggleSound} aria-pressed={sound} aria-label={sound ? "Mute game sound" : "Unmute game sound"} title={sound ? "Sound on" : "Sound off"}>
            {sound ? "♪ ON" : "♪ OFF"}
          </button>
        </div>
        {playing && <>
          <button
            className="hero-game-switch"
            aria-label={paused ? "Resume game" : "Pause game"}
            onClick={() => {
              changePause(!paused);
              stageRef.current?.focus();
            }}
          >
            {paused ? "RESUME" : "PAUSE"}
          </button>
          <button
            className="hero-game-switch"
            aria-label="Back to arcade menu"
            onClick={() => { setPlaying(false); setPaused(false); }}
          >
            MENU
          </button>
        </>}
      </div>
      <div ref={stageRef} tabIndex={0} className="hero-game-stage" aria-label={`${definition.title} playfield`} onPointerDown={onPointerDown} onPointerUp={onPointerUp} onPointerCancel={() => (pointerStart.current = null)}>
        {!playing && definition.renderAttract ? definition.renderAttract(palette, state) : definition.render(state, palette)}
        {playing && paused && (
          <div className="hero-game-pause" role="status">
            <span>PAUSED</span>
            <small>press any key</small>
          </div>
        )}
      </div>
      {entry && (
        <form className="hero-game-entry" onSubmit={submitEntry}>
          <input
            ref={entryRef}
            className="hero-game-entry-input"
            type="text"
            value={entryText}
            aria-label={entry.label}
            placeholder={entry.placeholder}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            onChange={(e) => setEntryText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") stageRef.current?.focus();
            }}
          />
          <button type="submit" className="hero-game-switch">ANSWER ↵</button>
        </form>
      )}
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
