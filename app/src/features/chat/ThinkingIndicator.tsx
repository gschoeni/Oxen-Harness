// The desktop twin of the CLI spinner: while the model works, cycle the active
// theme's spinner glyphs and rotate through its "thinking" phrases (or its
// write_file verbs when text is already streaming), with the same elapsed
// timer — `✶ Fording the river… (7s)` instead of three anonymous dots.
import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../../lib/store";
import {
  FRAME_MS,
  elapsedLabel,
  glyphAt,
  phraseAt,
  spinnerGlyphs,
  thinkingPhrases,
  writingPhrases,
} from "../../lib/thinking";

export function ThinkingIndicator({
  writing = false,
  trailing = false,
}: {
  /** Text is already streaming — speak in the theme's "writing" verbs. */
  writing?: boolean;
  /** Sits under streamed text (own line) rather than alone in the bubble. */
  trailing?: boolean;
}) {
  const theme = useStore((s) => s.theme);
  const phrases = useMemo(
    () => (writing ? writingPhrases(theme) : thinkingPhrases(theme)),
    [theme, writing],
  );
  const glyphs = useMemo(() => spinnerGlyphs(theme), [theme]);

  // Open on a random phrase (like the CLI's seeded start) and remember when we
  // appeared, so the timer reads from the start of this wait.
  const startIndex = useRef(Math.floor(Math.random() * phrases.length));
  const startedAt = useRef(Date.now());
  const [frame, setFrame] = useState(0);

  useEffect(() => {
    const id = window.setInterval(() => setFrame((f) => f + 1), FRAME_MS);
    return () => window.clearInterval(id);
  }, []);

  const phrase = phraseAt(phrases, startIndex.current, frame);
  const glyph = glyphAt(glyphs, frame);

  return (
    <span className={`thinking ${trailing ? "trailing" : ""}`}>
      <span className="thinking-glyph" aria-hidden>
        {glyph}
      </span>
      {/* Keyed so each new phrase replays the entrance animation. */}
      <span className="thinking-phrase" key={phrase}>
        {phrase}…
      </span>
      <span className="thinking-elapsed">({elapsedLabel(Date.now() - startedAt.current)})</span>
    </span>
  );
}
