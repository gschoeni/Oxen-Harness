// The Compression settings subpage: choose how outbound requests are shrunk.
// The agent resends the whole conversation on every model call, and most of
// its bulk is stale tool output; compression trims that on the wire only — the
// stored conversation is never modified. Off sends requests as recorded, audit
// measures would-be savings without changing anything, on compresses (with
// originals retrievable via the `retrieve_original` tool). The mode persists
// to compression.json, applies to the live chat immediately, and is the default
// for new chats. The composer's CompressionPicker offers the same switch inline.

import { useEffect, useState } from "react";
import { ChevronDown } from "lucide-react";
import { getCompressionMode, setCompressionMode, totalTokensSaved } from "../../lib/ipc";
import type { CompressionMode } from "../../lib/types";
import "../tools/tools.css";

/** The three modes, in escalation order, with the copy shown under the select. */
const MODES: { value: CompressionMode; label: string; blurb: string }[] = [
  {
    value: "off",
    label: "Off",
    blurb: "Requests are sent exactly as recorded.",
  },
  {
    value: "audit",
    label: "Audit",
    blurb:
      "Measures what compression would save on every request, without changing anything — watch the savings counter to decide.",
  },
  {
    value: "on",
    label: "On",
    blurb:
      "Stale tool output is compressed before each request; the model can retrieve any original via the retrieve_original tool; the stored conversation is never modified.",
  },
];

export function CompressionPage() {
  // null until the persisted mode loads, so the select never flashes a default.
  const [mode, setMode] = useState<CompressionMode | null>(null);
  const [saved, setSaved] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getCompressionMode()
      .then(setMode)
      .catch((e) => setError(String(e)));
    totalTokensSaved()
      .then(setSaved)
      .catch(() => {});
  }, []);

  // Optimistically switch, then persist; re-read on failure to resync.
  async function change(next: CompressionMode) {
    setMode(next);
    setError(null);
    try {
      await setCompressionMode(next);
    } catch (e) {
      setError(String(e));
      getCompressionMode()
        .then(setMode)
        .catch(() => {});
    }
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Context compression</div>
        <p className="hint">
          Every model call resends the whole conversation, and most of its bulk is stale tool
          output the model has already acted on. Compression shrinks that output{" "}
          <em>on the wire only</em> — your stored chats are untouched. Changes apply to the{" "}
          <strong>current chat immediately</strong> and become the default for new ones; you can
          also switch modes from the composer, next to the model picker.
        </p>
        {error && <span className="save-status err">{error}</span>}

        <label className="field">
          <span className="field-name">Mode</span>
          <span className="tool-select">
            <select
              className="tool-input"
              value={mode ?? "off"}
              disabled={mode === null}
              onChange={(e) => change(e.target.value as CompressionMode)}
              aria-label="Compression mode"
            >
              {MODES.map((m) => (
                <option key={m.value} value={m.value}>
                  {m.label}
                </option>
              ))}
            </select>
            <ChevronDown size={15} />
          </span>
        </label>

        {MODES.map((m) => (
          <p className="hint" key={m.value}>
            <strong>{m.label}</strong> — {m.blurb}
          </p>
        ))}
      </section>

      <section className="settings-section">
        <div className="settings-label">Savings</div>
        <div className="meta">
          <div className="meta-row">
            <span className="meta-key">tokens saved (all time)</span>
            <span className="meta-val">{saved === null ? "—" : saved.toLocaleString()}</span>
          </div>
        </div>
        <p className="hint">
          Estimated tokens kept off the wire across every chat. In audit mode this counts what{" "}
          <em>would</em> have been saved, so you can size up the win before turning
          compression on.
        </p>
      </section>
    </div>
  );
}
