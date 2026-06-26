import { describe, expect, it } from "vitest";
import { extractStringField, langForPath, partialCanvasDoc, partialFileWrite } from "./streamingArgs";

describe("extractStringField", () => {
  it("reads a complete string field", () => {
    expect(extractStringField('{"path":"src/main.rs","contents":"fn main(){}"}', "path")).toBe(
      "src/main.rs",
    );
  });

  it("reads an in-progress (unterminated) string at the stream edge", () => {
    expect(extractStringField('{"contents":"fn main() {\\n    let x = 1', "contents")).toBe(
      "fn main() {\n    let x = 1",
    );
  });

  it("decodes escape sequences, including unicode", () => {
    expect(extractStringField('{"s":"a\\tb\\n\\"c\\" \\u0041"}', "s")).toBe('a\tb\n"c" A');
  });

  it("tolerates an escape cut off mid-stream", () => {
    // Trailing lone backslash (escape not yet complete) is dropped, not crashed.
    expect(extractStringField('{"s":"line\\', "s")).toBe("line");
  });

  it("returns null when the field/opening quote hasn't arrived yet", () => {
    expect(extractStringField('{"path":', "path")).toBeNull();
    expect(extractStringField('{"other":"x"}', "path")).toBeNull();
  });
});

describe("partialFileWrite", () => {
  it("extracts write_file content + language from the path", () => {
    const w = partialFileWrite("write_file", '{"path":"a/b.ts","contents":"const x = 1"}');
    expect(w).toEqual({ verb: "Writing", path: "a/b.ts", content: "const x = 1", language: "typescript" });
  });

  it("extracts edit_file new_string as the content", () => {
    const w = partialFileWrite("edit_file", '{"path":"a.py","old_string":"x","new_string":"y = 2"}');
    expect(w?.verb).toBe("Editing");
    expect(w?.content).toBe("y = 2");
    expect(w?.language).toBe("python");
  });

  it("returns null for non-file tools", () => {
    expect(partialFileWrite("run_shell", '{"command":"ls"}')).toBeNull();
  });
});

describe("partialCanvasDoc", () => {
  it("builds a provisional doc from partial canvas args", () => {
    const doc = partialCanvasDoc('{"title":"Notes","format":"markdown","content":"# Hi\\nbody"}');
    expect(doc).toMatchObject({ title: "Notes", format: "markdown", content: "# Hi\nbody" });
    expect(doc?.id).toBe("notes");
  });

  it("returns null before any content has streamed", () => {
    expect(partialCanvasDoc('{"title":"Notes","format":"markdown"')).toBeNull();
  });
});

describe("langForPath", () => {
  it("maps extensions to hljs languages", () => {
    expect(langForPath("main.rs")).toBe("rust");
    expect(langForPath("a/b/c.tsx")).toBe("typescript");
    expect(langForPath("Cargo.toml")).toBe("ini");
    expect(langForPath("noext")).toBeUndefined();
  });
});
