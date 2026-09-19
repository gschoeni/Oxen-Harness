import { describe, expect, it } from "vitest";
import { parseGraph, connectionError, preset, type NodeKind } from "./graph";
const kinds: NodeKind[] = [
  { kind: "prompt", title: "", description: "", inputs: {}, output: "text" },
  { kind: "rewrite", title: "", description: "", inputs: { prompt: "text" }, output: "text" },
  {
    kind: "image",
    title: "",
    description: "",
    inputs: { prompt: "text", image: "image" },
    output: "image",
  },
];
describe("portable workflows", () => {
  it("retains extension metadata and unknown nodes for future modules", () => {
    const graph = preset("image");
    graph.custom = { a: 1 };
    graph.nodes[0].kind = "future-node";
    expect(parseGraph(JSON.stringify(graph))).toEqual(graph);
  });
  it("rejects versions, duplicate identities and malformed node positions without rewriting", () => {
    const graph = preset("image");
    graph.nodes.push(graph.nodes[0]);
    expect(() => parseGraph(JSON.stringify(graph))).toThrow("duplicate");
    expect(() => parseGraph('{"version":99}')).toThrow("Unsupported");
  });
  it("rejects type mismatches, occupied ports, and cycles", () => {
    const graph = preset("image");
    expect(connectionError(graph, "prompt", "image", "image", kinds)).toMatch(/different types/);
    expect(connectionError(graph, "prompt", "image", "prompt", kinds)).toMatch(/already/);
    graph.nodes[0].kind = "rewrite";
    expect(connectionError(graph, "rewrite", "prompt", "prompt", kinds)).toMatch(/cycle/);
  });
});
