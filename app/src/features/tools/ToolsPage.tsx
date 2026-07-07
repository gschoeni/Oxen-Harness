// The Tools settings subpage: manage which tools the agent may call, override
// the description the model sees for each, and inspect its JSON schema.
// Users can also add their own tools — a name, a description, and an HTTP
// endpoint that receives the model's arguments as JSON — and edit them later
// (see ToolEditor). Changes persist to tools.json and apply to new (and
// resumed) chats — surfaced in a hint so the behavior isn't surprising.

import { useEffect, useState } from "react";
import { ChevronRight, Globe, Plus, RotateCcw, Wrench } from "lucide-react";
import { Button } from "../../components/ui";
import {
  addCustomTool,
  listTools,
  removeCustomTool,
  setToolDescription,
  setToolEnabled,
} from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { CustomToolSpec, ToolInfo } from "../../lib/types";
import { ToolEditor } from "./ToolEditor";
import { ToolSwitch } from "./ToolSwitch";
import "./tools.css";

export function ToolsPage() {
  const setPage = useStore((s) => s.setSettingsPage);
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
                <Globe size={15} className="tool-row-icon" />
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
              <Plus size={15} />
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

        <p className="hint">
          Tools are what the agent can <em>do</em>. For what it should <em>know how to do</em> —
          reusable workflows loaded on demand through a single <code>skill</code> tool — see{" "}
          <button className="hint-link" onClick={() => setPage("skills")}>
            Skills
          </button>
          .
        </p>
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
        <ChevronRight size={15} className={`chevron ${open ? "open" : ""}`} />
        <Wrench size={15} className="tool-row-icon" />
        <span className="tool-row-name">{tool.name}</span>
        {overridden && <span className="tool-row-badge">edited</span>}
        {!open && <span className="tool-row-desc">{tool.description}</span>}
      </button>

      <ToolSwitch name={tool.name} enabled={tool.enabled} onToggle={onToggle} />

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
                <RotateCcw size={15} /> Reset to default
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
        <ChevronRight size={15} className={`chevron ${open ? "open" : ""}`} />
        <Globe size={15} className="tool-row-icon" />
        <span className="tool-row-name">{tool.name}</span>
        {!open && <span className="tool-row-desc">{tool.description}</span>}
      </button>

      <ToolSwitch name={tool.name} enabled={tool.enabled} onToggle={onToggle} />

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
