import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { Workflow, WandSparkles, Image, Film, Maximize, Type, ArrowDownToLine } from "lucide-react";
import type { GraphNode, NodeKind, Run } from "./graph";

export const icons: Record<string, typeof Workflow> = {
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
export const nodeTypes = { oxen: NodeCard };
export type FlowNode = Node<
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
