// The empty-state hero. A theme's `style.hero` picks the treatment — the same
// content (wordmark, tagline, status, prompts) rendered three very different
// ways: an 8-bit trail screen, a broadsheet masthead, or a clean app splash.

import { useStore } from "../../lib/store";
import { formatUsd } from "../../lib/format";
import { DEFAULT_HERO_GAME, HeroGame } from "./heroGames";
import { getScene } from "./scenes";
import { StatusPanel } from "./StatusPanel";

const DEFAULT_GAME_PALETTE = {
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

interface HeroProps {
  examples: string[];
  busy: boolean;
  onPick: (text: string) => void;
}

function Chips({ examples, busy, onPick }: HeroProps) {
  return (
    <div className="examples">
      {examples.map((ex) => (
        <button key={ex} className="example-chip" onClick={() => onPick(ex)} disabled={busy}>
          {ex}
        </button>
      ))}
    </div>
  );
}

export function Hero(props: HeroProps) {
  const theme = useStore((s) => s.theme);
  const heroGame = useStore((s) => s.heroGame);
  const setHeroGame = useStore((s) => s.setHeroGame);
  // The hero's "Total tokens used" shows the all-time grand total across every
  // session, so a fresh chat opens with your running tally rather than 0.
  const tokensUsed = useStore((s) => s.totalTokensUsed);
  // Estimated dollars spent at the current model's rates, shown right under the
  // token total. `null` when unavailable (local/unlisted model, or offline).
  const costUsd = useStore((s) => s.totalCostUsd);
  const v = theme?.voice;
  const hero = theme?.style?.hero || "pixel";

  const wordmark = v?.wordmark || "OXEN TRAIL";
  const preTagline = v?.pre_tagline ?? "～ The ～";
  const subtitle = v?.subtitle || "an open source agentic coding harness · powered by Oxen.ai";
  const bottomHint = v?.bottom_hint || "Press RETURN to size up the situation";
  const hint = bottomHint === "Send a message to begin on your trail" ? "" : bottomHint;
  const icon = v?.prompt_icon || "🐂";
  // Themes carry a static "Total tokens used" flavor row; swap in the live count
  // for the current session so the dashboard reflects real consumption, and
  // inject a "Total dollars spent" row right after it (dropping any static one
  // the theme carries, since the live value replaces it).
  const statusRows: [string, string][] = [
    ...(v?.flavor_top || []),
    ...(v?.flavor_bottom || []),
  ]
    .filter(([label]) => label !== "Total dollars spent")
    .flatMap(([label, value]): [string, string][] => {
      if (label !== "Total tokens used") return [[label, value]];
      const tokenRow: [string, string] = [label, `${tokensUsed.toLocaleString()} tokens`];
      const costRow: [string, string] = [
        "Total dollars spent",
        costUsd == null ? "—" : formatUsd(costUsd),
      ];
      return [tokenRow, costRow];
    });

  if (hero === "newspaper") {
    return (
      <div className="hero hero-news">
        <div className="news-rule news-rule-top" />
        <div className="news-dateline">
          <span>{preTagline}</span>
          <span className="news-dot">✦</span>
          <span>Late Edition</span>
        </div>
        <h1 className="hero-wordmark masthead">{wordmark}</h1>
        <div className="news-rule news-rule-bottom" />
        <p className="news-deck">{subtitle}</p>
        {statusRows.length > 0 && (
          <div className="news-index">
            <span className="news-index-head">Today's Index</span>
            <StatusPanel rows={statusRows} />
          </div>
        )}
        {hint && <p className="hero-hint">{hint}</p>}
        <Chips {...props} />
      </div>
    );
  }

  if (hero === "minimal") {
    return (
      <div className="hero hero-min">
        <div className="hero-glyph">{icon}</div>
        <h1 className="hero-wordmark">{wordmark}</h1>
        <p className="hero-sub">{subtitle}</p>
        {statusRows.length > 0 && <StatusPanel rows={statusRows} />}
        <Chips {...props} />
        {hint && <p className="hero-hint hero-hint-min">{hint}</p>}
      </div>
    );
  }

  // "pixel" — a framed retro "screen". Themes can still opt into the older
  // static scene renderer with `scene = "trail" | "grid"`; otherwise the empty
  // state is an interactive game, registered in heroGames.tsx.
  const palette = theme?.palette || DEFAULT_GAME_PALETTE;
  // The player's explicit choice wins; otherwise fall back to the theme's default
  // game (a theme can opt out of games entirely with `game = "none"`).
  const themeGame = typeof theme?.style?.game === "string" ? theme.style.game : undefined;
  const gameName = heroGame ?? themeGame;
  const useStaticScene = gameName === "none";
  const Scene = useStaticScene ? getScene(theme?.style?.scene) : null;
  return (
    <div className="hero">
      {preTagline && <p className="hero-pretag">{preTagline}</p>}
      <h1 className="hero-wordmark">{wordmark}</h1>
      <div className="hero-screen hero-game-screen">
        {useStaticScene && Scene ? (
          <>
            <Scene p={palette} />
            {hint && (
              <div className="hero-prompt">
                {hint}
                <span className="pixel-caret" aria-hidden="true" />
              </div>
            )}
          </>
        ) : (
          <HeroGame
            gameName={gameName ?? DEFAULT_HERO_GAME}
            palette={palette}
            hint={hint}
            onSelectGame={setHeroGame}
          />
        )}
      </div>
      <StatusPanel rows={statusRows} />
      <p className="hero-sub">{subtitle}</p>
      <Chips {...props} />
    </div>
  );
}
