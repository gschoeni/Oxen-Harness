/** Applying a harness *theme* to the live document: palette → accent tokens,
 *  `[style]` → typography + framing tokens. These are pure DOM side effects
 *  (they set CSS custom properties on :root), kept out of the state store so the
 *  store stays about state and the visual system lives in one place. */

import { lighten, readableOn, withAlpha } from "./color";
import type { Theme } from "./types";

/** Map the active harness theme's accent palette onto the app's accent tokens.
 *  Neutrals stay controlled by light/dark mode, so themes layer cleanly. */
export function applyThemePalette(theme: Theme) {
  const p = theme.palette;
  const root = document.documentElement.style;
  root.setProperty("--accent", p.primary);
  root.setProperty("--accent-hover", lighten(p.primary, 0.12));
  root.setProperty("--accent-soft", withAlpha(p.primary, 0.16));
  root.setProperty("--on-accent", readableOn(p.primary));
  root.setProperty("--focus", withAlpha(p.primary, 0.5));
  if (p.link) root.setProperty("--link", p.link);
  if (p.danger) root.setProperty("--danger", p.danger);
}

/** Map the active theme's `[style]` onto the typography + framing tokens, so the
 *  same components re-skin (8-bit trail, newspaper, soft Apple app, neon grid)
 *  from data alone. Missing fields keep the Oregon Trail defaults in pixel.css. */
export function applyThemeStyle(theme: Theme) {
  const st = theme.style;
  const root = document.documentElement.style;
  if (!st) return;

  root.setProperty("--font-display", st.font_display);
  root.setProperty("--font-body", st.font_body);
  root.setProperty("--font-readout", st.font_mono);
  root.setProperty("--frame-radius", st.radius);
  root.setProperty("--frame-border-w", st.border_width);
  root.setProperty("--label-transform", st.display_transform === "uppercase" ? "uppercase" : "none");
  root.setProperty("--label-spacing", st.display_spacing);
  root.setProperty("--wordmark-spacing", st.display_spacing);

  // A pixel hero needs the squat block face kept small; everything else gets a
  // larger, more conventional display size.
  const pixelHero = st.hero === "pixel";
  root.setProperty("--label-size", pixelHero ? "9px" : "11px");
  root.setProperty(
    "--wordmark-size",
    st.hero === "pixel"
      ? "clamp(15px, 4.4vw, 24px)"
      : st.hero === "newspaper"
        ? "clamp(30px, 8vw, 54px)"
        : "clamp(26px, 6.5vw, 40px)",
  );

  // Depth treatment → the three frame-shadow tokens + hover lift.
  const edge = "var(--pixel-edge)";
  let shadow = "none";
  let hover = "none";
  let lg = "none";
  let lift = "0px";
  switch (st.shadow) {
    case "pixel":
      shadow = `3px 3px 0 ${edge}`;
      hover = `4px 4px 0 ${edge}`;
      lg = `5px 5px 0 ${edge}`;
      lift = "-1px";
      break;
    case "soft":
      shadow = "0 1px 2px rgba(0,0,0,.10), 0 6px 20px rgba(0,0,0,.10)";
      hover = "0 2px 6px rgba(0,0,0,.12), 0 14px 36px rgba(0,0,0,.16)";
      lg = "0 16px 48px rgba(0,0,0,.18)";
      lift = "-1px";
      break;
    case "glow":
      shadow = "0 0 16px -4px var(--accent), 0 2px 0 rgba(0,0,0,.4)";
      hover = "0 0 24px -2px var(--accent), 0 2px 0 rgba(0,0,0,.4)";
      lg = "0 0 30px -4px var(--accent), 0 3px 0 rgba(0,0,0,.4)";
      lift = "-1px";
      break;
    case "none":
    default:
      break;
  }
  root.setProperty("--frame-shadow", shadow);
  root.setProperty("--frame-shadow-hover", hover);
  root.setProperty("--frame-shadow-lg", lg);
  root.setProperty("--lift", lift);

  // A coarse hook for hero-specific CSS (newspaper rules, minimal splash, …).
  document.documentElement.dataset.hero = st.hero;
}
