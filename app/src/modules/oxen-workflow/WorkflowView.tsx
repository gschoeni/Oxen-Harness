import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  Controls,
  MiniMap,
  Handle,
  Position,
  useReactFlow,
  type Node,
  type NodeProps,
  type NodeChange,
  type Connection,
} from "@xyflow/react";
import {
  Play,
  Square,
  Save,
  Plus,
  Workflow,
  WandSparkles,
  Image,
  Film,
  Maximize,
  Type,
  ArrowDownToLine,
  FileCode2,
  Trash2,
} from "lucide-react";
import { useDocument, type ViewProps, type WorkbenchAPI } from "../../workbench-sdk";
import {
  parseGraph,
  connectionError,
  preset,
  type Graph,
  type GraphNode,
  type NodeKind,
  type Model,
  type Run,
  type Output,
} from "./graph";
import "@xyflow/react/dist/style.css";
import "./workflow.css";

const MIME = "application/x-oxen-workflow-node";
const icons: Record<string, typeof Workflow> = {
  prompt: Type,
  rewrite: WandSparkles,
  image: Image,
  video: Film,
  upscale: Maximize,
  video_upscale: Maximize,
  image_input: Image,
  video_input: Film,
  output: ArrowDownToLine,
};
const nodeTypes = { oxen: NodeCard };
type FlowNode = Node<
  { node: GraphNode; definition?: NodeKind; result?: Run["nodes"][string] },
  "oxen"
>;

function NodeCard({ data, selected }: NodeProps<FlowNode>) {
  const { node, definition, result } = data,
    Icon = icons[node.kind] ?? Workflow;
  const inputs = Object.entries(definition?.inputs ?? {});
  return (
    <div
      className={`oxen-node${selected ? " selected" : ""}`}
      data-kind={node.kind}
      data-status={result?.status}
    >
      <div className="oxen-node-head">
        <Icon size={15} />
        <strong>{node.title || definition?.title || node.kind}</strong>
        <span>
          {result?.status === "running" ? "●" : result?.status === "succeeded" ? "✓" : ""}
        </span>
      </div>
      <div className="oxen-node-body">
        {node.kind === "prompt" ? (
          <p>{String(node.config?.text || "Write a prompt…")}</p>
        ) : (
          <small>
            {String(
              node.config?.model ||
                node.config?.path ||
                definition?.description ||
                "Unknown node — preserved in file",
            )}
          </small>
        )}
        {result?.error && <p className="oxen-node-error">{result.error}</p>}
        {result?.outputs?.some((o) => o.kind !== "text") && (
          <small>
            {result.outputs.length} saved output{result.outputs.length === 1 ? "" : "s"}
          </small>
        )}
      </div>
      <div className="oxen-node-ports">
        {inputs.map(([port, kind]) => (
          <div className="oxen-port" key={port}>
            <Handle type="target" position={Position.Left} id={port} data-kind={kind} />
            <span>{port}</span>
            <small>{kind}</small>
          </div>
        ))}
        {definition?.output !== "none" && (
          <div className="oxen-port output">
            <span>{definition?.output ?? "unknown"}</span>
            <Handle
              type="source"
              position={Position.Right}
              id="output"
              data-kind={definition?.output}
            />
          </div>
        )}
      </div>
    </div>
  );
}

export function WorkflowView({ api }: ViewProps) {
  const path = api.context.target.path;
  if (!path?.toLowerCase().endsWith(".graph.json")) return <CreateWorkflow api={api} />;
  return (
    <ReactFlowProvider key={`${api.context.workspace}:${path}`}>
      <WorkflowEditor api={api} path={path} />
    </ReactFlowProvider>
  );
}

function CreateWorkflow({ api }: { api: WorkbenchAPI }) {
  const [name, setName] = useState("workflows/studio.graph.json"),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  async function create(kind: "image" | "video" | "blank") {
    setBusy(true);
    setError("");
    try {
      const path = name.trim().endsWith(".graph.json") ? name.trim() : `${name.trim()}.graph.json`;
      if (!name.trim()) throw new Error("Choose a file name");
      await api.save(path, JSON.stringify(preset(kind), null, 2) + "\n");
      api.open({ view: "workflow", path });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="workbench-welcome workflow-welcome">
      <span className="workflow-eyebrow">OXEN WORKFLOWS</span>
      <Workflow size={36} />
      <h2>
        Ideas become images.
        <br />
        Images become motion.
      </h2>
      <p>
        Connect Oxen models into a workflow you can shape by hand or build with your agent. Every
        node and connection lives in a project file.
      </p>
      <label>
        Workflow file
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="workflows/studio.graph.json"
        />
      </label>
      <div className="workbench-choices">
        <button disabled={busy} onClick={() => void create("image")}>
          <Image size={20} />
          <strong>Image studio</strong>
          <span>Prompt → rewrite → image → upscale</span>
        </button>
        <button disabled={busy} onClick={() => void create("video")}>
          <Film size={20} />
          <strong>Image to motion</strong>
          <span>Turn an idea into a polished image and animate it.</span>
        </button>
        <button disabled={busy} onClick={() => void create("blank")}>
          <Plus size={20} />
          <strong>Start empty</strong>
          <span>Build your own pipeline from Oxen nodes.</span>
        </button>
      </div>
      <p>
        Creating a workflow is free. Run it when you’re ready; generation uses your Oxen account and
        media spend approvals.
      </p>
      {error && <p role="alert">{error}</p>}
    </div>
  );
}

function WorkflowEditor({ api, path }: { api: WorkbenchAPI; path: string }) {
  const doc = useDocument(api, path);
  const [kinds, setKinds] = useState<NodeKind[]>([]),
    [models, setModels] = useState<Model[]>([]);
  const [error, setError] = useState(""),
    [selected, setSelected] = useState<string>(),
    [palette, setPalette] = useState(false);
  const [run, setRun] = useState<Run>(),
    [starting, setStarting] = useState(false),
    [cancelling, setCancelling] = useState(false);
  const flow = useReactFlow<FlowNode>();
  const parsed = useMemo(() => {
    try {
      return { graph: parseGraph(doc.content) };
    } catch (e) {
      return { error: String(e) };
    }
  }, [doc.content]);
  const graph = parsed.graph;
  const working = starting || run?.status === "running" || run?.status === "queued";
  useEffect(() => {
    let live = true;
    void api
      .request<{ nodes: NodeKind[] }>("list")
      .then((v) => {
        if (live) setKinds(v.nodes);
      })
      .catch((e) => {
        if (live) setError(String(e));
      });
    void api
      .request<Model[]>("models")
      .then((v) => {
        if (live) setModels(v);
      })
      .catch((e) => {
        if (live) setError(`Model catalog: ${String(e)}`);
      });
    void api
      .request<Run | null>("latest", { path })
      .then((v) => {
        if (live && v) setRun(v);
      })
      .catch((e) => {
        if (live) setError(String(e));
      });
    return () => {
      live = false;
    };
  }, [api, path]);
  useEffect(() => {
    if (!run || !["running", "queued"].includes(run.status)) return;
    let live = true,
      timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await api.request<Run>("status", { id: run.id });
        if (live) {
          setRun(next);
          if (["running", "queued"].includes(next.status))
            timer = setTimeout(() => void poll(), 1200);
        }
      } catch (e) {
        if (live) {
          setError(String(e));
          timer = setTimeout(() => void poll(), 3000);
        }
      }
    };
    timer = setTimeout(() => void poll(), 800);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [api, run?.id, run?.status]);
  useEffect(() => {
    api.report({
      status: graph ? "mounted" : "invalid",
      dirty: doc.dirty,
      selection: selected ? [selected] : [],
      run_id: run?.id,
      revision: doc.snapshot?.revision,
    });
  }, [api, selected, doc.dirty, doc.snapshot?.revision, !!graph, run?.id]);
  const update = useCallback(
    (next: Graph) => doc.edit(JSON.stringify(next, null, 2) + "\n"),
    [doc.edit],
  );
  const nodes: FlowNode[] = useMemo(
    () =>
      graph?.nodes.map((node) => ({
        id: node.id,
        type: "oxen",
        position: node.position,
        selected: node.id === selected,
        data: {
          node,
          definition: kinds.find((k) => k.kind === node.kind),
          result: run?.nodes[node.id],
        },
      })) ?? [],
    [graph, kinds, run, selected],
  );
  const edges = useMemo(
    () =>
      graph?.edges.map((edge) => ({
        id: edge.id,
        source: edge.source,
        target: edge.target,
        sourceHandle: edge.source_port ?? "output",
        targetHandle: edge.target_port,
        animated: run?.nodes[edge.target]?.status === "running",
      })) ?? [],
    [graph, run],
  );
  function add(kind: string, position?: { x: number; y: number }) {
    if (!graph || graph.nodes.length >= 128) return;
    const id = `${kind}-${crypto.randomUUID().slice(0, 8)}`;
    const config: Record<string, unknown> =
      kind === "prompt"
        ? { text: "" }
        : kind === "upscale"
          ? { model: "flux-image-upscaler", params: { upscale_factor: 2 } }
          : kind === "video_upscale"
            ? { model: "flux-video-upscaler", params: { upscale_factor: 2 } }
            : {};
    const viewport = flow.getViewport();
    update({
      ...graph,
      nodes: [
        ...graph.nodes,
        {
          id,
          kind,
          config,
          position: position ?? {
            x: (80 - viewport.x) / viewport.zoom,
            y: (100 - viewport.y) / viewport.zoom,
          },
        },
      ],
    });
    setSelected(id);
    setPalette(false);
  }
  function changeNodes(changes: NodeChange<FlowNode>[]) {
    if (!graph) return;
    let next = graph;
    for (const change of changes) {
      if (change.type === "position" && change.position)
        next = {
          ...next,
          nodes: next.nodes.map((n) =>
            n.id === change.id ? { ...n, position: change.position! } : n,
          ),
        };
      if (change.type === "remove")
        next = {
          ...next,
          nodes: next.nodes.filter((n) => n.id !== change.id),
          edges: next.edges.filter((e) => e.source !== change.id && e.target !== change.id),
        };
      if (change.type === "select" && change.selected) setSelected(change.id);
    }
    if (next !== graph) update(next);
  }
  function connect(connection: Connection) {
    if (!graph) return;
    const port = connection.targetHandle ?? "input",
      issue = connectionError(graph, connection.source, connection.target, port, kinds);
    if (issue) {
      setError(issue);
      return;
    }
    update({
      ...graph,
      edges: [
        ...graph.edges,
        {
          id: `edge-${crypto.randomUUID()}`,
          source: connection.source,
          target: connection.target,
          source_port: "output",
          target_port: port,
        },
      ],
    });
    setError("");
  }
  async function save() {
    try {
      await doc.save();
      setError("");
    } catch (e) {
      setError(String(e));
    }
  }
  async function buildWithAgent() {
    try {
      if (doc.dirty) await doc.save();
      api.addToChat(
        `Help me build or improve the Oxen workflow in ${path}. Read list_views for the graph schema, then edit the file. Do not run it until I ask.`,
      );
    } catch (e) {
      setError(String(e));
    }
  }
  async function start() {
    setStarting(true);
    setError("");
    setCancelling(false);
    try {
      const saved = doc.dirty ? await doc.save() : doc.snapshot;
      if (!saved) throw new Error("Load and save the graph before running");
      setRun(await api.request<Run>("run", { path, revision: saved.revision }));
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const current = graph?.nodes.find((n) => n.id === selected);
  return (
    <div className="workflow-view">
      <header className="workflow-toolbar">
        <div>
          <strong>{graph?.title ?? "Workflow"}</strong>
          <span>
            {doc.dirty ? "Unsaved changes" : "Saved to project"}
            {working ? " · Running" : ""}
          </span>
        </div>
        <button onClick={() => setPalette((v) => !v)} aria-expanded={palette}>
          <Plus size={14} />
          Node
        </button>
        <button
          disabled={!doc.dirty || doc.saving || !!doc.conflict}
          onClick={() => void save()}
          title="Save workflow (⌘S)"
        >
          <Save size={14} />
        </button>
        {working ? (
          <button
            disabled={cancelling || starting}
            onClick={() => {
              if (run) {
                void api
                  .request("cancel", { id: run.id })
                  .then(() => setCancelling(true))
                  .catch((e) => setError(String(e)));
              }
            }}
          >
            <Square size={13} />
            {cancelling ? "Stopping…" : "Stop"}
          </button>
        ) : (
          <button
            className="workflow-run"
            disabled={!graph?.nodes.length || !!doc.conflict || !kinds.length}
            onClick={() => void start()}
          >
            <Play size={13} />
            Run
          </button>
        )}
      </header>
      {(error || doc.error) && (
        <div className="workbench-error" role="alert">
          {error || doc.error}
          <button
            className="icon-btn sm"
            aria-label="Dismiss workflow error"
            onClick={() => setError("")}
          >
            ×
          </button>
        </div>
      )}
      {doc.conflict && (
        <div className="workbench-conflict" role="alert">
          This graph changed on disk while you were editing. Your draft is preserved.
          <div>
            <button onClick={() => doc.resolve("disk")}>Use disk version</button>
            <button onClick={() => doc.resolve("draft")}>Keep my draft</button>
            <button
              onClick={() =>
                api.addToChat(`Help merge my unsaved workflow edits with ${path}:\n${doc.content}`)
              }
            >
              Ask agent to merge
            </button>
          </div>
        </div>
      )}
      {!graph ? (
        <div className="workbench-welcome">
          <p role="alert">{doc.snapshot ? parsed.error : "Loading workflow…"}</p>
          <button onClick={() => api.open({ view: "editor", path })}>
            <FileCode2 size={14} />
            Open source
          </button>
        </div>
      ) : (
        <>
          <div
            className="workflow-stage"
            onDragOver={(e) => {
              if (e.dataTransfer.types.includes(MIME)) {
                e.preventDefault();
                e.dataTransfer.dropEffect = "copy";
              }
            }}
            onDrop={(e) => {
              const kind = e.dataTransfer.getData(MIME);
              if (kinds.some((k) => k.kind === kind)) {
                e.preventDefault();
                add(kind, flow.screenToFlowPosition({ x: e.clientX, y: e.clientY }));
              }
            }}
          >
            <ReactFlow<FlowNode>
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onNodesChange={changeNodes}
              onEdgesChange={(changes) => {
                const removed = new Set(
                  changes.filter((c) => c.type === "remove").map((c) => c.id),
                );
                if (removed.size)
                  update({ ...graph, edges: graph.edges.filter((e) => !removed.has(e.id)) });
              }}
              onConnect={connect}
              onPaneClick={() => setSelected(undefined)}
              onNodeClick={(_, node) => setSelected(node.id)}
              fitView
              fitViewOptions={{ padding: 0.15, minZoom: 0.65, maxZoom: 0.9 }}
              minZoom={0.15}
              maxZoom={1.8}
              deleteKeyCode={["Backspace", "Delete"]}
              proOptions={{ hideAttribution: true }}
            >
              <Background gap={22} size={1} />
              <Controls showInteractive={false} />
              <MiniMap pannable zoomable nodeColor="var(--accent)" />
            </ReactFlow>
            {!graph.nodes.length && (
              <div className="workflow-empty">
                <Workflow size={28} />
                <strong>A blank canvas for your ideas</strong>
                <span>Add a node or ask your agent to build this workflow.</span>
                <button onClick={() => setPalette(true)}>
                  <Plus size={14} />
                  Add your first node
                </button>
              </div>
            )}
            {palette && (
              <div className="workflow-palette" aria-label="Add workflow node">
                <strong>OXEN NODES</strong>
                <span>Drag onto the canvas, or click to add.</span>
                {kinds.map((kind) => {
                  const Icon = icons[kind.kind] ?? Workflow;
                  return (
                    <button
                      key={kind.kind}
                      draggable
                      onDragStart={(e) => {
                        e.dataTransfer.setData(MIME, kind.kind);
                        e.dataTransfer.effectAllowed = "copy";
                      }}
                      onClick={() => add(kind.kind)}
                    >
                      <Icon size={16} />
                      <div>
                        <strong>{kind.title}</strong>
                        <small>{kind.description}</small>
                      </div>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
          {current && (
            <NodeInspector
              key={current.id}
              node={current}
              definition={kinds.find((k) => k.kind === current.kind)}
              models={models}
              outputs={run?.nodes[current.id]?.outputs ?? []}
              api={api}
              onChange={(node) =>
                update({ ...graph, nodes: graph.nodes.map((n) => (n.id === node.id ? node : n)) })
              }
              onRemove={() => {
                update({
                  ...graph,
                  nodes: graph.nodes.filter((n) => n.id !== current.id),
                  edges: graph.edges.filter(
                    (e) => e.source !== current.id && e.target !== current.id,
                  ),
                });
                setSelected(undefined);
              }}
            />
          )}
        </>
      )}
      <footer className="workflow-footer">
        <span>
          {graph?.nodes.length ?? 0} nodes · {graph?.edges.length ?? 0} connections
        </span>
        <button onClick={() => void buildWithAgent()}>Build with agent</button>
        <button onClick={() => api.open({ view: "editor", path })}>JSON</button>
      </footer>
      {run && (
        <div className="workflow-run-status" role="status">
          <strong>Run {run.status}</strong>
          <span>
            {Object.values(run.nodes).filter((n) => n.status === "succeeded").length} nodes
            completed
            {run.graph_revision !== doc.snapshot?.revision ? " · earlier graph revision" : ""}
          </span>
          {run.error && <p>{run.error}</p>}
          <button
            onClick={() =>
              api.open({ view: "editor", path: `.oxen-harness/workflow-runs/${run.id}.json` })
            }
          >
            Run record
          </button>
          <button onClick={() => api.open({ view: "gallery" })}>Open gallery</button>
          {cancelling && working && (
            <p>Finishing the current generation; downstream nodes will be skipped.</p>
          )}
        </div>
      )}
    </div>
  );
}

function NodeInspector({
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
