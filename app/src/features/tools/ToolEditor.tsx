// The create/edit form for a custom HTTP tool: name, description, endpoint,
// and a parameter builder. Parameters edit as friendly rows (name / type /
// description / required) that compile to the JSON Schema the model receives,
// with a raw-JSON mode for schemas the simple builder can't represent.

import { useState } from "react";
import { ChevronDown, Plus, Trash2 } from "lucide-react";
import { Button } from "../../components/ui";
import type { CustomToolSpec } from "../../lib/types";

/** One argument row in the simple parameter builder. */
export interface ParamRow {
  name: string;
  type: "string" | "number" | "boolean";
  description: string;
  required: boolean;
}

const PARAM_TYPES: { value: ParamRow["type"]; label: string }[] = [
  { value: "string", label: "text" },
  { value: "number", label: "number" },
  { value: "boolean", label: "yes / no" },
];

const EMPTY_ROW: ParamRow = { name: "", type: "string", description: "", required: false };

/** Compile builder rows into the JSON Schema the model receives. Blank rows
 *  (no name) are dropped so the starter row costs nothing. */
export function compileParams(rows: ParamRow[]): Record<string, unknown> {
  const kept = rows.filter((r) => r.name.trim() !== "");
  return {
    type: "object",
    properties: Object.fromEntries(
      kept.map((r) => [
        r.name.trim(),
        { type: r.type, ...(r.description.trim() ? { description: r.description.trim() } : {}) },
      ]),
    ),
    required: kept.filter((r) => r.required).map((r) => r.name.trim()),
  };
}

/** Try to view a JSON Schema as builder rows. Returns null when the schema uses
 *  anything beyond flat primitive properties — those edit as JSON instead. */
export function decomposeParams(schema: unknown): ParamRow[] | null {
  if (typeof schema !== "object" || schema === null || Array.isArray(schema)) return null;
  const s = schema as Record<string, unknown>;
  if (s.type !== "object") return null;
  for (const key of Object.keys(s)) {
    if (!["type", "properties", "required"].includes(key)) return null;
  }
  const props = s.properties ?? {};
  if (typeof props !== "object" || props === null || Array.isArray(props)) return null;
  const required = Array.isArray(s.required) ? s.required : [];
  const rows: ParamRow[] = [];
  for (const [name, def] of Object.entries(props as Record<string, unknown>)) {
    if (typeof def !== "object" || def === null || Array.isArray(def)) return null;
    const d = def as Record<string, unknown>;
    if (d.type !== "string" && d.type !== "number" && d.type !== "boolean") return null;
    for (const key of Object.keys(d)) {
      if (!["type", "description"].includes(key)) return null;
    }
    rows.push({
      name,
      type: d.type,
      description: typeof d.description === "string" ? d.description : "",
      required: required.includes(name),
    });
  }
  return rows;
}

/** The create/edit form for a custom tool. `initial` present = editing (name
 *  changes become a rename; a delete action appears). */
export function ToolEditor({
  initial,
  onSave,
  onCancel,
  onDelete,
}: {
  initial?: CustomToolSpec;
  onSave: (spec: CustomToolSpec) => Promise<void>;
  onCancel?: () => void;
  onDelete?: () => Promise<void>;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [url, setUrl] = useState(initial?.action.url ?? "");

  // Parameters edit as friendly rows when the schema is simple enough,
  // otherwise (or on demand) as raw JSON.
  const initialRows = initial ? decomposeParams(initial.parameters) : [{ ...EMPTY_ROW }];
  const [rows, setRows] = useState<ParamRow[]>(initialRows ?? []);
  const [jsonMode, setJsonMode] = useState(initialRows === null);
  const [jsonDraft, setJsonDraft] = useState(() =>
    JSON.stringify(initial?.parameters ?? compileParams(initialRows ?? []), null, 2),
  );

  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const canSave = name.trim() !== "" && description.trim() !== "" && url.trim() !== "";

  function switchMode(toJson: boolean) {
    setError(null);
    if (toJson) {
      setJsonDraft(JSON.stringify(compileParams(rows), null, 2));
      setJsonMode(true);
      return;
    }
    // Back to the builder only when the JSON still fits the simple shape.
    try {
      const decomposed = decomposeParams(JSON.parse(jsonDraft));
      if (decomposed === null) {
        setError("This schema uses features the simple editor can't show — keep editing as JSON.");
        return;
      }
      setRows(decomposed.length > 0 ? decomposed : [{ ...EMPTY_ROW }]);
      setJsonMode(false);
    } catch {
      setError("Fix the JSON before switching back to the simple editor.");
    }
  }

  function updateRow(i: number, patch: Partial<ParamRow>) {
    setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  }

  async function save() {
    setError(null);
    let parameters: unknown;
    if (jsonMode) {
      try {
        parameters = JSON.parse(jsonDraft);
      } catch {
        setError("Parameters aren't valid JSON.");
        return;
      }
    } else {
      const names = rows.map((r) => r.name.trim()).filter((n) => n !== "");
      if (new Set(names).size !== names.length) {
        setError("Two parameters share a name — parameter names must be unique.");
        return;
      }
      parameters = compileParams(rows);
    }

    setSaving(true);
    try {
      await onSave({
        name: name.trim(),
        description: description.trim(),
        parameters,
        action: { kind: "http_post", url: url.trim() },
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function reallyDelete() {
    if (!onDelete) return;
    setSaving(true);
    try {
      await onDelete();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  }

  return (
    <div className="tool-editor">
      <div className="tool-editor-grid">
        <label className="tool-field">
          <span className="field-name">Name</span>
          <input
            className="tool-input mono"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="lookup_customer"
            spellCheck={false}
          />
          <span className="tool-field-hint">How the model calls it — lowercase, underscores.</span>
        </label>
        <label className="tool-field">
          <span className="field-name">Endpoint URL</span>
          <input
            className="tool-input mono"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://api.example.com/lookup"
            spellCheck={false}
          />
          <span className="tool-field-hint">Arguments arrive here as a JSON POST body.</span>
        </label>
      </div>

      <label className="tool-field">
        <span className="field-name">Description</span>
        <textarea
          className="tool-desc-edit"
          rows={2}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="What does this tool do, and when should the model use it?"
        />
      </label>

      <div className="tool-field">
        <div className="tool-params-head">
          <span className="field-name">Parameters</span>
          <div className="tool-mode-toggle" role="group" aria-label="Parameters editor mode">
            <button className={jsonMode ? "" : "active"} onClick={() => switchMode(false)}>
              Simple
            </button>
            <button className={jsonMode ? "active" : ""} onClick={() => switchMode(true)}>
              JSON
            </button>
          </div>
        </div>

        {jsonMode ? (
          <textarea
            className="tool-desc-edit mono"
            rows={8}
            value={jsonDraft}
            onChange={(e) => setJsonDraft(e.target.value)}
            spellCheck={false}
            aria-label="Parameters JSON Schema"
          />
        ) : (
          <div className="tool-params">
            {rows.map((r, i) => (
              <div className="tool-param-row" key={i}>
                <input
                  className="tool-input mono"
                  value={r.name}
                  onChange={(e) => updateRow(i, { name: e.target.value })}
                  placeholder="name"
                  aria-label={`Parameter ${i + 1} name`}
                  spellCheck={false}
                />
                <span className="tool-select">
                  <select
                    className="tool-input"
                    value={r.type}
                    onChange={(e) => updateRow(i, { type: e.target.value as ParamRow["type"] })}
                    aria-label={`Parameter ${i + 1} type`}
                  >
                    {PARAM_TYPES.map((t) => (
                      <option key={t.value} value={t.value}>
                        {t.label}
                      </option>
                    ))}
                  </select>
                  <ChevronDown size={14} />
                </span>
                <input
                  className="tool-input"
                  value={r.description}
                  onChange={(e) => updateRow(i, { description: e.target.value })}
                  placeholder="What is this?"
                  aria-label={`Parameter ${i + 1} description`}
                />
                <label className="tool-param-required" title="The model must always provide this">
                  <input
                    type="checkbox"
                    checked={r.required}
                    onChange={(e) => updateRow(i, { required: e.target.checked })}
                    aria-label={`Parameter ${i + 1} required`}
                  />
                  req
                </label>
                <button
                  className="tool-param-remove"
                  onClick={() => setRows((rs) => rs.filter((_, j) => j !== i))}
                  title="Remove parameter"
                  aria-label={`Remove parameter ${i + 1}`}
                >
                  <Trash2 size={13} />
                </button>
              </div>
            ))}
            <button
              className="tool-param-add"
              onClick={() => setRows((rs) => [...rs, { ...EMPTY_ROW }])}
            >
              <Plus size={14} /> Add parameter
            </button>
          </div>
        )}
      </div>

      {error && <span className="save-status err">{error}</span>}

      <div className="tool-row-actions tool-editor-actions">
        <Button variant="primary" size="sm" disabled={!canSave || saving} onClick={save}>
          {saving ? "Saving…" : initial ? "Save changes" : "Add tool"}
        </Button>
        {onCancel && (
          <Button variant="ghost" size="sm" onClick={onCancel} disabled={saving}>
            Cancel
          </Button>
        )}
        {onDelete &&
          (confirmDelete ? (
            <Button variant="danger" size="sm" onClick={reallyDelete} disabled={saving}>
              Really delete?
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setConfirmDelete(true)}
              title="Remove this tool"
            >
              <Trash2 size={14} /> Delete tool
            </Button>
          ))}
      </div>
    </div>
  );
}
