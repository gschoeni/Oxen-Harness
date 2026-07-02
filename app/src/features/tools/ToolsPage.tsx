// The Tools settings subpage: manage which built-in tools the agent may call,
// override the description the model sees for each, and inspect its JSON schema.
// Users can also add their own tools — a name, a description, and an HTTP
// endpoint that receives the model's arguments as JSON — and edit them later.
// Changes persist to tools.json and apply to new (and resumed) chats — surfaced
// in a hint so the behavior isn't surprising.

import { useEffect, useState } from "react";
import { ChevronRight, Globe, Plus, RotateCcw, Trash2, Wrench } from "lucide-react";
import { Button } from "../../components/ui";
import {
  addCustomTool,
  listTools,
  removeCustomTool,
  setToolDescription,
  setToolEnabled,
} from "../../lib/ipc";
import type { CustomToolSpec, ToolInfo } from "../../lib/types";
import "./tools.css";

export function ToolsPage() {
  const [tools, setTools] = useState<ToolInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Whether the "new tool" editor card is open.
  const [adding, setAdding] = useState(false);

  const load = () =>
    listTools()
      .then(setTools)
      .catch((e) => setError(String(e)));

  useEffect(() => {
    load();
  }, []);

  // Optimistically flip the toggle, then persist; reload on failure to resync.
  async function toggle(name: string, enabled: boolean) {
    setTools((ts) => ts?.map((t) => (t.name === name ? { ...t, enabled } : t)) ?? ts);
    try {
      await setToolEnabled(name, enabled);
    } catch (e) {
      setError(String(e));
      load();
    }
  }

  async function saveDescription(name: string, description: string | null) {
    try {
      await setToolDescription(name, description);
      await load();
    } catch (e) {
      setError(String(e));
    }
  }

  // Create or update a custom tool. A rename is add-then-remove (add first, so
  // a rejected new name can't lose the existing tool). Throws so the editor can
  // show the backend's message inline.
  async function saveCustom(spec: CustomToolSpec, prevName?: string) {
    await addCustomTool(spec);
    if (prevName && prevName !== spec.name) await removeCustomTool(prevName);
    await load();
  }

  async function deleteCustom(name: string) {
    await removeCustomTool(name);
    await load();
  }

  const builtins = tools?.filter((t) => t.builtin) ?? [];
  const custom = tools?.filter((t) => !t.builtin) ?? [];
  const enabledCount = builtins.filter((t) => t.enabled).length;

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">
          Your tools{tools && custom.length > 0 && ` · ${custom.length}`}
        </div>
        <p className="hint">
          Give the agent new abilities without writing code: name a tool, describe when to use
          it, and point it at an HTTP endpoint. The model's arguments are sent as a JSON POST
          body and the response comes back as the tool result. Changes apply to{" "}
          <strong>new and resumed chats</strong>.
        </p>
        {error && <span className="save-status err">{error}</span>}

        <div className="tool-list">
          {custom.map((t) => (
            <CustomToolRow
              key={t.name}
              tool={t}
              onToggle={toggle}
              onSave={saveCustom}
              onDelete={deleteCustom}
            />
          ))}

          {adding ? (
            <div className="tool-row tool-row-new">
              <div className="tool-editor-title">
                <Globe size={14} className="tool-row-icon" />
                New tool
              </div>
              <ToolEditor
                onSave={async (spec) => {
                  await saveCustom(spec);
                  setAdding(false);
                }}
                onCancel={() => setAdding(false)}
              />
            </div>
          ) : (
            <button className="tool-add" onClick={() => setAdding(true)} disabled={tools === null}>
              <Plus size={16} />
              New tool
            </button>
          )}
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-label">
          Built-in tools{tools && ` · ${enabledCount}/${builtins.length} on`}
        </div>
        <p className="hint">
          Turn capabilities on or off, or reword how a tool is described to the model.
        </p>

        <div className="tool-list">
          {tools === null ? (
            <p className="muted">Loading tools…</p>
          ) : (
            builtins.map((t) => (
              <ToolRow key={t.name} tool={t} onToggle={toggle} onSaveDescription={saveDescription} />
            ))
          )}
        </div>
      </section>
    </div>
  );
}

// ---- built-in tool rows -----------------------------------------------------

function ToolRow({
  tool,
  onToggle,
  onSaveDescription,
}: {
  tool: ToolInfo;
  onToggle: (name: string, enabled: boolean) => void;
  onSaveDescription: (name: string, description: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(tool.description);

  // Keep the editor in sync if the tool reloads (e.g. after a save elsewhere).
  useEffect(() => setDraft(tool.description), [tool.description]);

  const overridden = tool.description !== tool.default_description;
  const dirty = draft.trim() !== tool.description.trim();

  return (
    <div className={`tool-row ${tool.enabled ? "" : "disabled"}`}>
      <button className="tool-row-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRight size={14} className={`tool-chevron ${open ? "open" : ""}`} />
        <Wrench size={14} className="tool-row-icon" />
        <span className="tool-row-name">{tool.name}</span>
        {overridden && <span className="tool-row-badge">edited</span>}
        {!open && <span className="tool-row-desc">{tool.description}</span>}
      </button>

      <ToolSwitch tool={tool} onToggle={onToggle} />

      {open && (
        <div className="tool-row-body">
          <div className="field-name">Description shown to the model</div>
          <textarea
            className="tool-desc-edit"
            rows={3}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            spellCheck={false}
          />
          <div className="tool-row-actions">
            <Button
              variant="primary"
              size="sm"
              disabled={!dirty}
              onClick={() => onSaveDescription(tool.name, draft.trim())}
            >
              Save description
            </Button>
            {overridden && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onSaveDescription(tool.name, null)}
                title="Restore the built-in description"
              >
                <RotateCcw size={14} /> Reset to default
              </Button>
            )}
          </div>

          <details className="tool-schema">
            <summary>Parameters schema</summary>
            <pre className="tool-schema-code">{JSON.stringify(tool.parameters, null, 2)}</pre>
          </details>
        </div>
      )}
    </div>
  );
}

// ---- custom tool rows -------------------------------------------------------

/** A user-added tool: expands into the same editor used to create it. */
function CustomToolRow({
  tool,
  onToggle,
  onSave,
  onDelete,
}: {
  tool: ToolInfo;
  onToggle: (name: string, enabled: boolean) => void;
  onSave: (spec: CustomToolSpec, prevName: string) => Promise<void>;
  onDelete: (name: string) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const url = typeof tool.config.url === "string" ? tool.config.url : "";

  return (
    <div className={`tool-row ${tool.enabled ? "" : "disabled"}`}>
      <button className="tool-row-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRight size={14} className={`tool-chevron ${open ? "open" : ""}`} />
        <Globe size={14} className="tool-row-icon" />
        <span className="tool-row-name">{tool.name}</span>
        {!open && <span className="tool-row-desc">{tool.description}</span>}
      </button>

      <ToolSwitch tool={tool} onToggle={onToggle} />

      {open && (
        <div className="tool-row-body">
          <ToolEditor
            initial={{
              name: tool.name,
              // The spec's own description (default_description) is what we
              // edit; `description` may carry an override layered on top.
              description: tool.default_description,
              parameters: tool.parameters,
              action: { kind: "http_post", url },
            }}
            onSave={async (spec) => {
              await onSave(spec, tool.name);
              setOpen(false);
            }}
            onDelete={() => onDelete(tool.name)}
          />
        </div>
      )}
    </div>
  );
}

function ToolSwitch({
  tool,
  onToggle,
}: {
  tool: ToolInfo;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  return (
    <label
      className="tool-switch"
      title={tool.enabled ? "Enabled — click to turn off" : "Disabled — click to turn on"}
      onClick={(e) => e.stopPropagation()}
    >
      <input
        type="checkbox"
        checked={tool.enabled}
        onChange={(e) => onToggle(tool.name, e.target.checked)}
        aria-label={`${tool.enabled ? "Disable" : "Enable"} ${tool.name}`}
      />
      <span className="tool-switch-track" />
    </label>
  );
}

// ---- the tool editor (create + edit) ----------------------------------------

/** One argument row in the simple parameter builder. */
interface ParamRow {
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

/** Compile builder rows into the JSON Schema the model receives. Blank rows
 *  (no name) are dropped so the starter row costs nothing. */
function compileParams(rows: ParamRow[]): Record<string, unknown> {
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
function decomposeParams(schema: unknown): ParamRow[] | null {
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

const EMPTY_ROW: ParamRow = { name: "", type: "string", description: "", required: false };

/** The create/edit form for a custom tool. `initial` present = editing (name
 *  changes become a rename; a delete action appears). */
function ToolEditor({
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
