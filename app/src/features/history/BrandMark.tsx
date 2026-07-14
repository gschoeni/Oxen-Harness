// The app's mark (the themed ox). Shared by the expanded sidebar's brand row
// and the collapsed rail, so the logo holds the same spot in both states and
// nothing jumps when the panel collapses.

import { useStore } from "../../lib/store";

export function BrandMark() {
  const theme = useStore((s) => s.theme);
  return <span className="brand-icon">{theme?.voice.prompt_icon || "🐂"}</span>;
}
