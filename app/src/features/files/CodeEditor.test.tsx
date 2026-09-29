import { describe, expect, it, vi } from "vitest";
import { act, render } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { CodeEditor } from "./CodeEditor";

function viewIn(container: HTMLElement): EditorView {
  const dom = container.querySelector(".cm-editor");
  const view = dom && EditorView.findFromDOM(dom as HTMLElement);
  if (!view) throw new Error("editor not mounted");
  return view;
}

/** Type `text` at the end of the buffer, as a keypress would. */
function typeAtEnd(view: EditorView, text: string) {
  const end = view.state.doc.length;
  view.dispatch({ changes: { from: end, insert: text }, selection: { anchor: end + text.length } });
}

describe("CodeEditor", () => {
  it("keeps the same editor and cursor when the parent echoes an edit back as value", () => {
    // The pane stores each keystroke and re-renders with the new text; that
    // round trip must not rebuild the editor (which resets the cursor to 0).
    let value = "hello";
    const onChange = vi.fn((doc: string) => {
      value = doc;
    });
    const { container, rerender } = render(
      <CodeEditor value={value} filename="a.txt" onChange={onChange} />,
    );
    const view = viewIn(container);
    act(() => typeAtEnd(view, " world"));
    expect(onChange).toHaveBeenLastCalledWith("hello world");
    rerender(<CodeEditor value={value} filename="a.txt" onChange={onChange} />);
    expect(viewIn(container)).toBe(view);
    expect(view.state.doc.toString()).toBe("hello world");
    expect(view.state.selection.main.head).toBe("hello world".length);
  });

  it("replaces the buffer in place when value changes from outside", () => {
    // A reload from disk (agent edit, conflict resolved to disk) reaches the
    // editor as a new value; it should land without a rebuild or a stale doc.
    const onChange = vi.fn();
    const { container, rerender } = render(
      <CodeEditor value="draft" filename="a.txt" onChange={onChange} />,
    );
    const view = viewIn(container);
    view.dispatch({ selection: { anchor: 5 } });
    rerender(<CodeEditor value="on disk" filename="a.txt" onChange={onChange} />);
    expect(viewIn(container)).toBe(view);
    expect(view.state.doc.toString()).toBe("on disk");
    expect(view.state.selection.main.head).toBe(5);
    // The parent already holds this text; echoing it back would be a no-op at
    // best and a phantom edit on a read-only preview at worst.
    expect(onChange).not.toHaveBeenCalled();
    rerender(<CodeEditor value="" filename="a.txt" onChange={onChange} />);
    expect(view.state.doc.toString()).toBe("");
    expect(view.state.selection.main.head).toBe(0);
  });

  it("toggles read-only and wrapping without rebuilding", () => {
    const { container, rerender } = render(<CodeEditor value="x" filename="a.txt" />);
    const view = viewIn(container);
    expect(view.state.readOnly).toBe(false);
    rerender(<CodeEditor value="x" filename="a.txt" readOnly wrap />);
    expect(viewIn(container)).toBe(view);
    expect(view.state.readOnly).toBe(true);
    expect(view.contentDOM.classList.contains("cm-lineWrapping")).toBe(true);
  });
});
