// Settings → Media: the knobs behind `generate_image` / `generate_video`.
//
// Budgets are the trust dial: a generation whose estimated cost is at or
// under both limits runs without asking; anything over (or unpriced) asks in
// the chat first. "Always ask" clears a limit. Default models are what the
// agent reaches for when it doesn't name one; the output folder is where the
// files land, relative to the project. Every change saves at once and
// applies to new (and resumed) chats.

import { useEffect, useState } from "react";
import { Check } from "lucide-react";
import { getMediaPrefs, listMediaModels, setMediaPrefs } from "../../lib/ipc";
import type { MediaModelSummary, MediaPrefs } from "../../lib/types";
import { Select } from "../../components/ui/Select";
import type { SelectOption } from "../../components/ui/Select";
import { ToolSwitch } from "../tools/ToolSwitch";

export function MediaPage() {
  const [prefs, setPrefs] = useState<MediaPrefs | null>(null);
  const [imageModels, setImageModels] = useState<MediaModelSummary[] | null>(null);
  const [videoModels, setVideoModels] = useState<MediaModelSummary[] | null>(null);
  const [savedAt, setSavedAt] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getMediaPrefs()
      .then(setPrefs)
      .catch((e) => setError(String(e)));
    listMediaModels("image")
      .then(setImageModels)
      .catch(() => setImageModels([]));
    listMediaModels("video")
      .then(setVideoModels)
      .catch(() => setVideoModels([]));
  }, []);

  // Optimistic: show the change at once, revert if the save fails.
  const update = (patch: Partial<MediaPrefs>) => {
    if (!prefs) return;
    const previous = prefs;
    const next = { ...prefs, ...patch };
    setPrefs(next);
    setError(null);
    setMediaPrefs(next)
      .then(() => setSavedAt(Date.now()))
      .catch((e) => {
        setPrefs(previous);
        setError(String(e));
      });
  };

  if (!prefs) return <div className="settings-page">{error ? <p className="hint">{error}</p> : null}</div>;

  return (
    <div className="settings-page media-page">
      <section className="settings-section">
        <div className="settings-label">Budgets</div>
        <p className="hint">
          A generation estimated at or under both limits runs on its own. Anything over — or a model
          the catalog can't price — asks you in the chat first. Videos are billed per second of
          output, images per image.
        </p>
        <div className="media-budgets">
          <BudgetField
            id="media-per-generation"
            label="Per generation"
            hint="one image or clip"
            value={prefs.per_generation_usd}
            onChange={(v) => update({ per_generation_usd: v })}
          />
          <BudgetField
            id="media-per-run"
            label="Per run"
            hint="one tool call, up to 4 outputs"
            value={prefs.per_run_usd}
            onChange={(v) => update({ per_run_usd: v })}
          />
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-label">Default models</div>
        <p className="hint">
          What the agent uses when it doesn't name a model. It can still pick any model from the
          catalog for a specific job (a reference-to-video model, an upscaler).
        </p>
        <ModelField
          id="media-image-model"
          label="Image"
          value={prefs.default_image_model}
          models={imageModels}
          onChange={(v) => update({ default_image_model: v })}
        />
        <ModelField
          id="media-video-model"
          label="Video"
          value={prefs.default_video_model}
          models={videoModels}
          onChange={(v) => update({ default_video_model: v })}
        />
      </section>

      <section className="settings-section">
        <div className="settings-label">Output folder</div>
        <p className="hint">
          Where generated files are saved, relative to the project. The manifest beside them is
          what the Gallery reads.
        </p>
        <FolderField value={prefs.output_dir} onChange={(v) => update({ output_dir: v })} />
      </section>

      <section className="settings-section">
        <div className="settings-label">Keeping copies</div>
        <HubRepoField value={prefs.hub_repo} onChange={(v) => update({ hub_repo: v })} />
        <div className="preview-verify-row media-switch-row">
          <div className="preview-verify-text">
            <span className="preview-verify-title">Commit generations with Oxen</span>
            <p className="hint">When this project is an Oxen repo, each finished batch is added and committed.</p>
          </div>
          <ToolSwitch
            name="commit generations with Oxen"
            enabled={prefs.commit_with_oxen}
            onToggle={(_name, next) => update({ commit_with_oxen: next })}
          />
        </div>
      </section>

      <div className="media-save-state" aria-live="polite">
        {error ? (
          <span className="media-save-error">{error}</span>
        ) : savedAt ? (
          <span className="media-saved">
            <Check size={13} /> Saved — applies to new chats
          </span>
        ) : null}
      </div>
    </div>
  );
}

function BudgetField({
  id,
  label,
  hint,
  value,
  onChange,
}: {
  id: string;
  label: string;
  hint: string;
  value: number | null;
  onChange: (v: number | null) => void;
}) {
  const [draft, setDraft] = useState(value === null ? "" : String(value));
  useEffect(() => {
    setDraft(value === null ? "" : String(value));
  }, [value]);
  const alwaysAsk = value === null;
  const commit = () => {
    const n = Number.parseFloat(draft);
    if (Number.isFinite(n) && n >= 0) {
      if (n !== value) onChange(n);
    } else {
      setDraft(value === null ? "" : String(value));
    }
  };
  return (
    <div className="media-budget">
      <label className="media-field" htmlFor={id}>
        <span className="media-field-label">
          {label} <span className="media-field-hint">{hint}</span>
        </span>
        <span className="media-usd">
          <span aria-hidden="true">$</span>
          <input
            id={id}
            className="field-input"
            type="number"
            min={0}
            step={0.05}
            inputMode="decimal"
            value={draft}
            disabled={alwaysAsk}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
            }}
          />
        </span>
      </label>
      <label className="media-always-ask">
        <input
          type="checkbox"
          checked={alwaysAsk}
          onChange={(e) => onChange(e.target.checked ? null : 0.25)}
          aria-label={`Always ask before a ${label.toLowerCase()}`}
        />
        Always ask
      </label>
    </div>
  );
}

function ModelField({
  id,
  label,
  value,
  models,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  models: MediaModelSummary[] | null;
  onChange: (v: string) => void;
}) {
  const known = models && models.length > 0;
  // The saved model may not be in the fetched catalog (renamed, offline):
  // keep it selectable so the select never silently shows something else.
  const options: SelectOption[] = known
    ? [
        ...(models.some((m) => m.id === value) ? [] : [{ value, label: value }]),
        ...models.map((m) => ({ value: m.id, label: m.id, hint: m.price || undefined })),
      ]
    : [];
  return (
    <label className="media-field" htmlFor={known ? undefined : id}>
      <span className="media-field-label">{label}</span>
      {known ? (
        <Select label={label} value={value} options={options} onValueChange={onChange} searchable />
      ) : (
        <input
          id={id}
          className="field-input"
          value={value}
          spellCheck={false}
          placeholder={models === null ? "loading the catalog…" : "model id"}
          onChange={(e) => onChange(e.target.value)}
        />
      )}
    </label>
  );
}

function HubRepoField({ value, onChange }: { value: string | null; onChange: (v: string | null) => void }) {
  const [draft, setDraft] = useState(value ?? "");
  useEffect(() => {
    setDraft(value ?? "");
  }, [value]);
  const commit = () => {
    const v = draft.trim().replace(/^\/+|\/+$/g, "");
    const next = v === "" ? null : v;
    if (next !== value) onChange(next);
    else setDraft(value ?? "");
  };
  return (
    <div className="media-hub-repo">
      <label className="media-field" htmlFor="media-hub-repo">
        <span className="media-field-label">Hub repo</span>
        <input
          id="media-hub-repo"
          className="field-input"
          value={draft}
          spellCheck={false}
          placeholder="ox/my-generations"
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
          }}
        />
      </label>
      <p className="hint">
        Also store each generation in this Oxen repo on hub.oxen.ai, under the output folder. Leave
        empty to keep files only in the project.
      </p>
    </div>
  );
}

function FolderField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => {
    setDraft(value);
  }, [value]);
  const commit = () => {
    const v = draft.trim() || "generations";
    if (v !== value) onChange(v);
    else setDraft(value);
  };
  return (
    <label className="media-field" htmlFor="media-output-dir">
      <span className="media-field-label">Folder</span>
      <input
        id="media-output-dir"
        className="field-input"
        value={draft}
        spellCheck={false}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
      />
    </label>
  );
}
