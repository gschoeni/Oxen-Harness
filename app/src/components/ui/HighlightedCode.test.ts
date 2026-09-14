import { describe, expect, it } from "vitest";
import { GRAMMARS, loadHljs } from "./HighlightedCode";
import { PATH_LANGUAGES, langForPath } from "../../lib/streamingArgs";

describe("HighlightedCode: curated grammar set", () => {
  it("bundles a grammar for every language the extension map can name", () => {
    // A language mapped from a file extension but missing here would render
    // as plain text with no error — pin the two lists together.
    for (const lang of PATH_LANGUAGES) expect(GRAMMARS, lang).toHaveProperty(lang);
  });

  it("registers the curated set and the aliases the chat actually uses", async () => {
    const hljs = await loadHljs();
    // Everything the extension map produces resolves by name…
    for (const lang of PATH_LANGUAGES) expect(hljs.getLanguage(lang), lang).toBeDefined();
    // …and by the short aliases the model tends to write in fences.
    for (const alias of [
      "js", "jsx", "ts", "tsx", "py", "sh", "zsh", "shell", "rs", "yml", "md", "html", "svg",
      "toml", "cs", "kt", "cc", "hpp", "rb", "golang", "patch", "docker", "txt", "text", "plaintext",
      "jsonc", "vue",
    ]) {
      expect(hljs.getLanguage(alias), alias).toBeDefined();
    }
    // The map's own outputs go through the same resolver at render time.
    expect(hljs.getLanguage(langForPath("Cargo.toml")!)).toBeDefined();
    expect(hljs.getLanguage(langForPath("index.vue")!)).toBeDefined();
  });

  it("highlights a named language without auto-detection", async () => {
    const hljs = await loadHljs();
    const out = hljs.highlight("let x = 1;", { language: "rust" }).value;
    expect(out).toContain("hljs-keyword");
    // Nothing outside the curated set is resolvable — the full library is not
    // in the bundle.
    expect(hljs.getLanguage("fortran")).toBeUndefined();
    expect(hljs.listLanguages().length).toBeLessThan(40);
  });
});
