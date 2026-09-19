import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import type { WorkbenchAPI } from "../../workbench-sdk";
import type { GraphNode, NodeKind, Model, Output } from "./graph";

export function NodeInspector({
  node,
  definition,
  models,
  outputs,
  api,
  onChange,
  onRemove,
}: {
  node: GraphNode;
  definition?: NodeKind;
  models: Model[];
  outputs: Output[];
  api: WorkbenchAPI;
  onChange: (node: GraphNode) => void;
  onRemove: () => void;
}) {
  const config = node.config ?? {},
    text = (key: string) => String(config[key] ?? "");
  const update = (key: string, value: unknown) =>
    onChange({ ...node, config: { ...config, [key]: value } });
  const media = ["image", "video", "upscale", "video_upscale"].includes(node.kind),
    video = ["video", "video_upscale"].includes(node.kind);
  const model = models.find((m) => m.id === text("model"));
  return (
    <section className="workflow-inspector" aria-label="Node settings">
      <header>
        <strong>{definition?.title ?? node.kind}</strong>
        <code>{node.id}</code>
        <button className="icon-btn sm" aria-label="Delete selected node" onClick={onRemove}>
          <Trash2 size={14} />
        </button>
      </header>
      <div className="workflow-fields">
        <label>
          Label
          <input
            value={node.title ?? ""}
            placeholder={definition?.title}
            onChange={(e) => onChange({ ...node, title: e.target.value })}
          />
        </label>
        {node.kind === "prompt" && (
          <label className="wide">
            Prompt
            <textarea
              value={text("text")}
              onChange={(e) => update("text", e.target.value)}
              placeholder="Describe what you want to create…"
            />
          </label>
        )}
        {node.kind.endsWith("_input") && (
          <label className="wide">
            Project file
            <input
              value={text("path")}
              onChange={(e) => update("path", e.target.value)}
              placeholder="assets/reference.png"
            />
          </label>
        )}
        {node.kind === "rewrite" && (
          <>
            <label>
              Oxen language model
              <input
                value={text("model")}
                placeholder="Use default model"
                onChange={(e) => update("model", e.target.value)}
              />
            </label>
            <label className="wide">
              Rewrite instructions
              <textarea
                value={text("instructions")}
                placeholder="Make this a vivid, detailed generation prompt."
                onChange={(e) => update("instructions", e.target.value)}
              />
            </label>
          </>
        )}
        {media && (
          <>
            <label>
              Oxen model
              <select
                value={text("model")}
                onChange={(e) =>
                  onChange({ ...node, config: { ...config, model: e.target.value, params: {} } })
                }
              >
                <option value="">Project default</option>
                {models
                  .filter((m) => m.kind === (video ? "video" : "image"))
                  .map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.display_name ?? m.id}
                    </option>
                  ))}
              </select>
            </label>
            {["image", "video"].includes(node.kind) && (
              <label className="wide">
                Prompt fallback
                <textarea
                  value={text("prompt")}
                  onChange={(e) => update("prompt", e.target.value)}
                  placeholder="Used when no prompt node is connected"
                />
              </label>
            )}
            <Parameters
              model={model}
              values={(config.params ?? {}) as Record<string, unknown>}
              onChange={(values) => update("params", values)}
            />
          </>
        )}
      </div>
      {outputs.length > 0 && (
        <div className="workflow-outputs">
          {outputs.map((output, i) => (
            <OutputPreview key={`${output.value}:${i}`} output={output} api={api} />
          ))}
        </div>
      )}
    </section>
  );
}

const reserved = new Set([
  "prompt",
  "model",
  "num_generations",
  "target_namespace",
  "target_repo",
  "target_directory",
]);
function Parameters({
  model,
  values,
  onChange,
}: {
  model?: Model;
  values: Record<string, unknown>;
  onChange: (values: Record<string, unknown>) => void;
}) {
  if (!model) return null;
  return (
    <>
      {Object.entries(model.request_schema.properties ?? {})
        .filter(
          ([name, schema]) =>
            !reserved.has(name) &&
            schema.format !== "uri" &&
            ["string", "integer", "number", "boolean"].includes(String(schema.type)),
        )
        .map(([name, schema]) => {
          const value = values[name] ?? schema.default ?? "",
            type = String(schema.type);
          const change = (next: unknown) => {
            const copy = { ...values };
            if (next === "") delete copy[name];
            else copy[name] = next;
            onChange(copy);
          };
          return (
            <label key={name} title={String(schema.description ?? name)}>
              {name.replace(/_/g, " ")}
              {Array.isArray(schema.enum) ? (
                <select
                  value={String(value)}
                  onChange={(e) =>
                    change(
                      type === "number" || type === "integer"
                        ? Number(e.target.value)
                        : e.target.value,
                    )
                  }
                >
                  <option value="">Default</option>
                  {schema.enum.map((v) => (
                    <option key={String(v)} value={String(v)}>
                      {String(v)}
                    </option>
                  ))}
                </select>
              ) : type === "boolean" ? (
                <input
                  type="checkbox"
                  checked={value === true}
                  onChange={(e) => change(e.target.checked)}
                />
              ) : (
                <input
                  type={type === "number" || type === "integer" ? "number" : "text"}
                  value={String(value)}
                  min={typeof schema.minimum === "number" ? schema.minimum : undefined}
                  max={typeof schema.maximum === "number" ? schema.maximum : undefined}
                  step={type === "integer" ? 1 : "any"}
                  onChange={(e) =>
                    change(
                      e.target.value === ""
                        ? ""
                        : type === "string"
                          ? e.target.value
                          : Number(e.target.value),
                    )
                  }
                />
              )}
            </label>
          );
        })}
    </>
  );
}

function OutputPreview({ output, api }: { output: Output; api: WorkbenchAPI }) {
  const [src, setSrc] = useState<string>(),
    [error, setError] = useState("");
  useEffect(() => {
    if (output.kind === "text") return;
    let live = true;
    void api
      .asset(output.value)
      .then((v) => {
        if (live) setSrc(v);
      })
      .catch((e) => {
        if (live) setError(String(e));
      });
    return () => {
      live = false;
    };
  }, [api, output.kind, output.value]);
  return (
    <figure>
      {output.kind === "text" ? (
        <p>{output.value}</p>
      ) : src ? (
        output.kind === "video" ? (
          <video src={src} controls />
        ) : (
          <img src={src} alt="Workflow output" />
        )
      ) : null}
      {error && <p role="alert">{error}</p>}
      <figcaption>
        <button
          onClick={() =>
            output.kind === "text"
              ? api.addToChat(output.value)
              : api.open({ view: "editor", path: output.value })
          }
        >
          {output.kind === "text" ? "Add to chat" : output.value.split("/").pop()}
        </button>
      </figcaption>
    </figure>
  );
}
