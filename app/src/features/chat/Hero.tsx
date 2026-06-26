// The empty-state hero. A theme's `style.hero` picks the treatment — the same
// content (wordmark, tagline, status, prompts) rendered three very different
// ways: an 8-bit trail screen, a broadsheet masthead, or a clean app splash.

import { useStore } from "../../lib/store";
import { getScene } from "./scenes";
import { StatusPanel } from "./StatusPanel";

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
  // The hero's "Total tokens used" shows the all-time grand total across every
  // session, so a fresh chat opens with your running tally rather than 0.
  const tokensUsed = useStore((s) => s.totalTokensUsed);
  const v = theme?.voice;
  const hero = theme?.style?.hero || "pixel";

  const wordmark = v?.wordmark || "OXEN TRAIL";
  const preTagline = v?.pre_tagline ?? "～ The ～";
  const subtitle = v?.subtitle || "an open source agentic coding harness · powered by Oxen.ai";
  const hint = v?.bottom_hint || "Press RETURN to size up the situation";
  const icon = v?.prompt_icon || "🐂";
  // Themes carry a static "Total tokens used" flavor row; swap in the live count
  // for the current session so the dashboard reflects real consumption.
  const statusRows: [string, string][] = [...(v?.flavor_top || []), ...(v?.flavor_bottom || [])].map(
    ([label, value]) =>
      label === "Total tokens used" ? [label, `${tokensUsed.toLocaleString()} tokens`] : [label, value],
  );

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
        <p className="hero-hint">{hint}</p>
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
        <p className="hero-hint hero-hint-min">{hint}</p>
      </div>
    );
  }

  // "pixel" — a framed retro "screen" (the trail wagon, the synth grid, …).
  const Scene = getScene(theme?.style?.scene);
  const palette = theme?.palette;
  return (
    <div className="hero">
      {preTagline && <p className="hero-pretag">{preTagline}</p>}
      <h1 className="hero-wordmark">{wordmark}</h1>
      <div className="hero-screen">
        {Scene && palette && <Scene p={palette} />}
        <div className="hero-prompt">
          {hint}
          <span className="pixel-caret" aria-hidden="true" />
        </div>
      </div>
      <StatusPanel rows={statusRows} />
      <p className="hero-sub">{subtitle}</p>
      <Chips {...props} />
    </div>
  );
}
