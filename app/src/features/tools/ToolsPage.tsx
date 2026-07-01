// The Tools settings subpage: manage which built-in tools the agent may call,
// override the description the model sees for each, and inspect its JSON schema.
// Changes persist to tools.json and apply to new (and resumed) chats — surfaced
// in a hint so the behavior isn't surprising.

import { useEffect, useState } from "react";
import { ChevronRight, Plug, RotateCcw, Wrench } from "lucide-react";
import { Button } from "../../components/ui";
import { listTools, setToolDescription, setToolEnabled } from "../../lib/ipc";
import type { ToolInfo } from "../../lib/types";
import "./tools.css";

export function ToolsPage() {
  const [tools, setTools] = useState<ToolInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

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

  const enabledCount = tools?.filter((t) => t.enabled).length ?? 0;

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">
          Built-in tools{tools && ` · ${enabledCount}/${tools.length} on`}
        </div>
        <p className="hint">
          Turn capabilities on or off, or reword how a tool is described to the model.
          Changes apply to <strong>new and resumed chats</strong> — your current chat keeps
          the tools it started with.
        </p>
        {error && <span className="save-status err">{error}</span>}

        <div className="tool-list">
          {tools === null ? (
            <p className="muted">Loading tools…</p>
          ) : (
            tools.map((t) => (
              <ToolRow key={t.name} tool={t} onToggle={toggle} onSaveDescription={saveDescription} />
            ))
          )}
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-label">Custom & MCP tools</div>
        <div className="tool-soon">
          <Plug size={18} />
          <div>
            <div className="tool-soon-title">Connect your own tools</div>
            <p className="hint">
              Bring external capabilities to the agent via the Model Context Protocol or your
              own HTTP endpoints. Coming soon.
            </p>
          </div>
        </div>
      </section>
    </div>
  );
}

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
