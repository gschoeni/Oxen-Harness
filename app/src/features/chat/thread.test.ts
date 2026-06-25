import { describe, expect, it } from "vitest";
import {
  appendToken,
  finalizeAssistant,
  startTurn,
  toolEnd,
  toolStart,
  transcriptToItems,
  type Item,
} from "./thread";

const assistantText = (items: Item[]) =>
  items.filter((i): i is Extract<Item, { kind: "assistant" }> => i.kind === "assistant");

describe("thread: startTurn", () => {
  it("adds the user prompt and an empty in-flight assistant bubble", () => {
    const items = startTurn([], "hello");
    expect(items.map((i) => i.kind)).toEqual(["user", "assistant"]);
    expect(assistantText(items)[0]).toMatchObject({ text: "", streaming: true });
  });
});

describe("thread: appendToken", () => {
  it("accumulates tokens into the streaming assistant bubble", () => {
    let items = startTurn([], "hi");
    items = appendToken(items, "Hel");
    items = appendToken(items, "lo");
    expect(assistantText(items)[0].text).toBe("Hello");
  });
});

describe("thread: tools", () => {
  it("retires an empty thinking bubble when a tool starts, then resumes after", () => {
    let items = startTurn([], "go"); // [user, empty assistant]
    items = toolStart(items, "run_command", '{"command":"ls"}', 1000);
    expect(items.map((i) => i.kind)).toEqual(["user", "tool"]); // empty bubble dropped
    expect(items[1]).toMatchObject({ kind: "tool", running: true, args: '{"command":"ls"}' });

    items = toolEnd(items, "run_command", "ok", 4000);
    const tool = items.find((i) => i.kind === "tool") as Extract<Item, { kind: "tool" }>;
    expect(tool).toMatchObject({ running: false, result: "ok", endedAt: 4000 });
    // A fresh streaming bubble is opened for any text after the tool.
    expect(items[items.length - 1]).toMatchObject({ kind: "assistant", streaming: true });
  });

  it("keeps a non-empty assistant bubble when a tool starts", () => {
    let items = appendToken(startTurn([], "go"), "partial");
    items = toolStart(items, "read_file", "", 0);
    expect(items.map((i) => i.kind)).toEqual(["user", "assistant", "tool"]);
  });
});

describe("thread: finalizeAssistant", () => {
  it("keeps streamed text and stops streaming", () => {
    const items = finalizeAssistant(appendToken(startTurn([], "q"), "answer"), "ignored");
    expect(assistantText(items)[0]).toMatchObject({ text: "answer", streaming: false });
  });

  it("falls back to the returned final text when nothing streamed", () => {
    const items = finalizeAssistant(startTurn([], "q"), "final answer");
    expect(assistantText(items)[0]).toMatchObject({ text: "final answer", streaming: false });
  });

  it("drops an empty bubble when there is no text at all", () => {
    const items = finalizeAssistant(startTurn([], "q"), "");
    expect(assistantText(items)).toHaveLength(0);
  });

  it("marks errors so they can be styled", () => {
    const items = finalizeAssistant(startTurn([], "q"), "⚠ boom", true);
    expect(assistantText(items)[0]).toMatchObject({ error: true });
  });
});

describe("thread: transcriptToItems", () => {
  it("renders user/assistant turns and tool calls, skipping system + tool results", () => {
    const items = transcriptToItems([
      { role: "system", content: "be helpful" },
      { role: "user", content: "list files" },
      {
        role: "assistant",
        content: "Sure.",
        tool_calls: [{ id: "1", type: "function", function: { name: "run_command", arguments: "ls" } }],
      },
      { role: "tool", content: "a.ts b.ts", tool_call_id: "1" },
    ]);
    expect(items.map((i) => i.kind)).toEqual(["user", "assistant", "tool"]);
    expect(items[2]).toMatchObject({ kind: "tool", name: "run_command", running: false });
  });
});
