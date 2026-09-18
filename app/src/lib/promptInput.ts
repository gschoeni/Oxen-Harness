/** Code-oriented prompts shouldn't start native prediction/correction services. */
export const PROMPT_INPUT_PROPS = {
  spellCheck: false,
  autoCorrect: "off",
  autoCapitalize: "off",
  // This HTML attribute is not yet in our React types; spreading it preserves
  // the lowercase DOM spelling without augmenting every element's typings.
  writingsuggestions: "false",
} as const;

/** WebKit may identify the IME's final Enter only through keyCode 229. */
export function isComposingKey(event: KeyboardEvent): boolean {
  return event.isComposing || event.keyCode === 229;
}
