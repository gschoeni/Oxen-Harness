import { useEffect, useState } from "react";
import { Button, Modal } from "../../components/ui";
import {
  exportTheme,
  importTheme,
  listThemes,
  newTheme,
  removeTheme,
  useTheme,
} from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { ThemeSummary } from "../../lib/types";
import "./themes.css";

const MOODS = ["Cozy & warm", "Sleek & dark", "Loud & vibrant", "Minimal & calm"];
const COLORS = ["Sunset oranges", "Ocean blues", "Forest greens", "Neon magenta + cyan", "Monochrome"];
const VOICES = ["Playful & punny", "Professional & terse", "Epic & dramatic", "Chill & friendly"];

export function ThemesModal() {
  const setThemesOpen = useStore((s) => s.setThemesOpen);
  const applyTheme = useStore((s) => s.applyTheme);

  const [themes, setThemes] = useState<ThemeSummary[]>([]);
  const [status, setStatus] = useState<string>("");
  const [mood, setMood] = useState(MOODS[0]);
  const [colors, setColors] = useState(COLORS[0]);
  const [voice, setVoice] = useState(VOICES[0]);
  const [desc, setDesc] = useState("");
  const [importText, setImportText] = useState("");
  const [generating, setGenerating] = useState(false);

  const refresh = () => listThemes().then(setThemes).catch((e) => setStatus(String(e)));
  useEffect(() => {
    refresh();
  }, []);

  async function switchTo(name: string) {
    try {
      applyTheme(await useTheme(name));
      await refresh();
    } catch (e) {
      setStatus(`theme switch failed: ${e}`);
    }
  }

  async function doExport(name: string) {
    try {
      await navigator.clipboard.writeText(await exportTheme(name));
      setStatus(`Copied "${name}" to the clipboard — paste to share it.`);
    } catch (e) {
      setStatus(`export failed: ${e}`);
    }
  }

  async function doRemove(name: string) {
    try {
      await removeTheme(name);
      await refresh();
    } catch (e) {
      setStatus(`remove failed: ${e}`);
    }
  }

  async function generate() {
    setGenerating(true);
    setStatus("Designing your theme with the model…");
    try {
      const brief =
        "Create a complete terminal theme.\n" +
        `User's description: ${desc.trim() || "(no extra description)"}\n` +
        `Mood: ${mood}\n` +
        `Color inspiration: ${colors}\n` +
        `Voice/personality: ${voice}\n\n` +
        "Give it a short, evocative name. Make the palette cohesive and readable on a " +
        "dark UI, and write all the voice phrases (thinking, tool_verbs, deaths, " +
        "subtitle, labels) to match the mood and personality. Output the theme now.";
      const t = await newTheme(brief);
      applyTheme(t);
      await refresh();
      setStatus(`Created & activated "${t.meta.name}".`);
    } catch (e) {
      setStatus(`generation failed: ${e}`);
    } finally {
      setGenerating(false);
    }
  }

  async function doImport() {
    if (!importText.trim()) return;
    try {
      applyTheme(await importTheme(importText));
      await refresh();
      setImportText("");
      setStatus("Imported & activated.");
    } catch (e) {
      setStatus(`import failed: ${e}`);
    }
  }

  return (
    <Modal title="Themes" wide onClose={() => setThemesOpen(false)}>
      <div className="themes-list">
        {themes.map((t) => (
          <div className={`theme-row ${t.active ? "active" : ""}`} key={t.slug}>
            <span className="theme-marker">{t.active ? "●" : "○"}</span>
            <div className="theme-name">
              {t.name}
              <span className="theme-tag">{t.builtin ? "built-in" : "custom"}</span>
            </div>
            <div className="theme-actions">
              {!t.active && (
                <Button size="sm" onClick={() => switchTo(t.name)}>
                  Use
                </Button>
              )}
              <Button size="sm" variant="ghost" onClick={() => doExport(t.name)}>
                Export
              </Button>
              {!t.builtin && (
                <Button size="sm" variant="danger" onClick={() => doRemove(t.name)}>
                  Remove
                </Button>
              )}
            </div>
            <div className="theme-desc">{t.description}</div>
          </div>
        ))}
      </div>

      <details className="theme-panel">
        <summary>✨ Vibe-code a new theme</summary>
        <div className="theme-panel-body">
          <div className="theme-selects">
            <label>
              Mood
              <select value={mood} onChange={(e) => setMood(e.target.value)}>
                {MOODS.map((m) => (
                  <option key={m}>{m}</option>
                ))}
              </select>
            </label>
            <label>
              Colors
              <select value={colors} onChange={(e) => setColors(e.target.value)}>
                {COLORS.map((c) => (
                  <option key={c}>{c}</option>
                ))}
              </select>
            </label>
            <label>
              Voice
              <select value={voice} onChange={(e) => setVoice(e.target.value)}>
                {VOICES.map((v) => (
                  <option key={v}>{v}</option>
                ))}
              </select>
            </label>
          </div>
          <textarea
            rows={2}
            value={desc}
            onChange={(e) => setDesc(e.target.value)}
            placeholder="Describe your dream theme… (e.g. a cozy autumn cabin with warm ambers and pine greens)"
          />
          <Button variant="primary" onClick={generate} disabled={generating}>
            {generating ? "Generating…" : "Generate with the model"}
          </Button>
        </div>
      </details>

      <details className="theme-panel">
        <summary>📥 Import a shared theme</summary>
        <div className="theme-panel-body">
          <textarea
            rows={4}
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            placeholder="Paste a theme's TOML (or JSON) here…"
          />
          <Button variant="primary" onClick={doImport}>
            Import & activate
          </Button>
        </div>
      </details>

      {status && <div className="theme-status muted">{status}</div>}
    </Modal>
  );
}
