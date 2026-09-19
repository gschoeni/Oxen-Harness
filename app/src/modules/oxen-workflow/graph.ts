/** Versioned, portable file format. Unknown fields survive visual edits. */
export interface GraphNode {
  id: string;
  kind: string;
  title?: string;
  position: { x: number; y: number };
  config?: Record<string, unknown>;
  [key: string]: unknown;
}
export interface GraphEdge {
  id: string;
  source: string;
  target: string;
  source_port?: string;
  target_port: string;
  [key: string]: unknown;
}
export interface Graph {
  version: 1;
  title: string;
  nodes: GraphNode[];
  edges: GraphEdge[];
  [key: string]: unknown;
}
export interface NodeKind {
  kind: string;
  title: string;
  description: string;
  inputs: Record<string, string>;
  output: string;
}
export interface Model {
  id: string;
  kind: "image" | "video";
  display_name?: string;
  pricing?: unknown;
  request_schema: { properties?: Record<string, Record<string, unknown>>; required?: string[] };
}
export interface Output {
  kind: string;
  value: string;
  job?: string;
}
export interface Run {
  id: string;
  status: string;
  graph_revision: string;
  error?: string;
  nodes: Record<string, { status: string; outputs: Output[]; error?: string }>;
}

export function parseGraph(content: string): Graph {
  const value: unknown = JSON.parse(content);
  if (!value || typeof value !== "object") throw new Error("Workflow must be an object");
  const g = value as Graph;
  if (g.version !== 1) throw new Error(`Unsupported workflow version ${g.version}`);
  if (!Array.isArray(g.nodes) || !Array.isArray(g.edges) || typeof g.title !== "string")
    throw new Error("Workflow needs a title, nodes, and edges");
  if (g.nodes.length > 128 || g.edges.length > 512)
    throw new Error("Workflow limit: 128 nodes and 512 connections");
  const ids = new Set<string>();
  for (const n of g.nodes) {
    if (
      !n ||
      typeof n.id !== "string" ||
      !/^[\w.-]{1,80}$/.test(n.id) ||
      ids.has(n.id) ||
      typeof n.kind !== "string" ||
      !Number.isFinite(n.position?.x) ||
      !Number.isFinite(n.position?.y) ||
      (n.config !== undefined &&
        (!n.config || typeof n.config !== "object" || Array.isArray(n.config)))
    )
      throw new Error("A node has invalid or duplicate identity, position, or configuration");
    ids.add(n.id);
  }
  const edges = new Set<string>();
  for (const e of g.edges) {
    if (
      !e ||
      typeof e.id !== "string" ||
      edges.has(e.id) ||
      typeof e.source !== "string" ||
      typeof e.target !== "string" ||
      typeof e.target_port !== "string"
    )
      throw new Error("A connection is invalid or duplicated");
    edges.add(e.id);
  }
  return g;
}

export function connectionError(
  graph: Graph,
  source: string,
  target: string,
  port: string,
  kinds: NodeKind[],
) {
  const from = graph.nodes.find((n) => n.id === source),
    to = graph.nodes.find((n) => n.id === target);
  if (!from || !to) return "Connection has a missing node";
  const output = kinds.find((k) => k.kind === from.kind)?.output,
    input = kinds.find((k) => k.kind === to.kind)?.inputs[port];
  if (!input || !output || output === "none" || (input !== "any" && input !== output))
    return "These ports carry different types";
  if (input !== "any" && graph.edges.some((e) => e.target === target && e.target_port === port))
    return "This input already has a connection";
  const visited = new Set<string>(),
    pending = [target];
  while (pending.length) {
    const node = pending.pop()!;
    if (node === source) return "A connection cannot create a cycle";
    if (visited.has(node)) continue;
    visited.add(node);
    pending.push(...graph.edges.filter((e) => e.source === node).map((e) => e.target));
  }
  return null;
}

export function preset(kind: "image" | "video" | "blank"): Graph {
  const node = (
    id: string,
    kind: string,
    x: number,
    config: Record<string, unknown> = {},
  ): GraphNode => ({
    id,
    kind,
    position: { x: ((x / 300) % 2) * 300, y: Math.floor(x / 600) * 240 },
    config,
  });
  const edge = (source: string, target: string, target_port: string): GraphEdge => ({
    id: `${source}-${target}`,
    source,
    target,
    source_port: "output",
    target_port,
  });
  if (kind === "blank") return { version: 1, title: "Untitled workflow", nodes: [], edges: [] };
  const nodes = [
    node("prompt", "prompt", 0, {
      text: "A cinematic photograph of a tiny cabin above the clouds at sunrise, warm light, fine detail.",
    }),
    node("rewrite", "rewrite", 300),
    node("image", "image", 600),
    node("upscale", "upscale", 900, {
      model: "flux-image-upscaler",
      params: { upscale_factor: 2 },
    }),
  ];
  const edges = [
    edge("prompt", "rewrite", "prompt"),
    edge("rewrite", "image", "prompt"),
    edge("image", "upscale", "image"),
  ];
  if (kind === "video") {
    nodes.push(
      node("video", "video", 1200, {
        prompt: "Slow cinematic camera push in, clouds drifting naturally.",
      }),
    );
    edges.push(edge("upscale", "video", "image"));
  }
  nodes.push(node("output", "output", kind === "video" ? 1500 : 1200));
  edges.push(edge(kind === "video" ? "video" : "upscale", "output", "input"));
  return { version: 1, title: kind === "video" ? "Image to motion" : "Image studio", nodes, edges };
}
