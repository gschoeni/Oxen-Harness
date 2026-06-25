// Small color utilities for deriving accent tokens from a theme's palette.

/** Parse a `#rrggbb` string into [r, g, b] (0–255). */
export function parseHex(hex: string): [number, number, number] {
  const n = parseInt(hex.replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** Black or white text for a given background, by WCAG-ish luminance. */
export function readableOn(hex: string): string {
  const [r, g, b] = parseHex(hex);
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return lum > 0.62 ? "#1d1b16" : "#ffffff";
}

/** An `rgba()` string for `hex` at the given alpha (0–1). */
export function withAlpha(hex: string, alpha: number): string {
  const [r, g, b] = parseHex(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/** Mix `hex` toward white by `amount` (0–1), returning an `rgb()` string. */
export function lighten(hex: string, amount: number): string {
  const [r, g, b] = parseHex(hex);
  const f = (c: number) => Math.round(c + (255 - c) * amount);
  return `rgb(${f(r)}, ${f(g)}, ${f(b)})`;
}
