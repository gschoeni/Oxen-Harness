import { describe, expect, it } from "vitest";
import { compileParams, decomposeParams, type ParamRow } from "./ToolEditor";

const row = (patch: Partial<ParamRow>): ParamRow => ({
  name: "",
  type: "string",
  description: "",
  required: false,
  ...patch,
});

describe("compileParams / decomposeParams", () => {
  it("compiles rows to a JSON Schema object, dropping blank rows", () => {
    const schema = compileParams([
      row({ name: "email", description: "Customer email", required: true }),
      row({ name: "limit", type: "number" }),
      row({}), // the untouched starter row
    ]);
    expect(schema).toEqual({
      type: "object",
      properties: {
        email: { type: "string", description: "Customer email" },
        limit: { type: "number" },
      },
      required: ["email"],
    });
  });

  it("round-trips a compiled schema back into the same rows", () => {
    const rows = [
      row({ name: "email", description: "Customer email", required: true }),
      row({ name: "verbose", type: "boolean" }),
    ];
    expect(decomposeParams(compileParams(rows))).toEqual(rows);
  });

  it("refuses schemas the simple builder can't represent", () => {
    // Nested objects, enums, and non-object roots all fall back to JSON mode.
    expect(
      decomposeParams({
        type: "object",
        properties: { nested: { type: "object", properties: {} } },
      }),
    ).toBeNull();
    expect(
      decomposeParams({
        type: "object",
        properties: { mode: { type: "string", enum: ["a", "b"] } },
      }),
    ).toBeNull();
    expect(decomposeParams({ type: "array" })).toBeNull();
    expect(decomposeParams("not a schema")).toBeNull();
  });
});
