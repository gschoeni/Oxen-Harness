import { describe, expect, it } from "vitest";
import {
  appendApiKeyPrompt,
  appendNotice,
  appendToken,
  finalizeAssistant,
  resolveApiKeyPrompt,
  startTurn,
  toolEnd,
  toolStart,
  transcriptToItems,
  type Item,
} from "./thread";

const assistantText = (items: Item[]) =>
  items.filter((i): i is Extract<Item, { kind: "assistant" }> => i.kind === "assistant");

describe("thread: appendNotice", () => {
  it("inserts the notice before a trailing empty streaming bubble", () => {
    const items = startTurn([], "hello"); // [user, empty streaming assistant]
    const next = appendNotice(items, "Compacted context — pruned old output");
    expect(next.map((i) => i.kind)).toEqual(["user", "notice", "assistant"]);
    // The streaming bubble is preserved as the last item so the reply continues below.
    expect(next[next.length - 1]).toMatchObject({ kind: "assistant", streaming: true });
  });

  it("appends at the end when there is no in-flight bubble", () => {
    const items = finalizeAssistant(startTurn([], "hi"), "done");
    const next = appendNotice(items, "note");
    expect(next[next.length - 1]).toMatchObject({ kind: "notice", text: "note" });
  });
});

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

  it("finalizes a non-empty preamble bubble when a tool starts", () => {
    // e.g. the model says "I'll read that file…" then calls a tool.
    let items = appendToken(startTurn([], "go"), "I'll read that file");
    items = toolStart(items, "read_file", "", 0);
    expect(items.map((i) => i.kind)).toEqual(["user", "assistant", "tool"]);
    // The preamble bubble stops streaming so its activity indicator hands off to
    // the tool chip (rather than two indicators showing at once).
    const preamble = items[1] as Extract<Item, { kind: "assistant" }>;
    expect(preamble).toMatchObject({ text: "I'll read that file", streaming: false });
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

describe("thread: API-key prompt", () => {
  it("swaps the empty reply bubble for a key card carrying the failed turn", () => {
    const items = startTurn([], "Write me a README", ["/abs/a.png"]); // [user, empty assistant]
    const next = appendApiKeyPrompt(items, "Write me a README", ["/abs/a.png"]);
    expect(next.map((i) => i.kind)).toEqual(["user", "apikey"]); // empty bubble dropped
    expect(next[1]).toMatchObject({
      kind: "apikey",
      text: "Write me a README",
      attachments: ["/abs/a.png"],
    });
  });

  it("keeps any streamed preamble text when it swaps in the key card", () => {
    const items = appendToken(startTurn([], "go"), "One moment"); // non-empty streaming bubble
    const next = appendApiKeyPrompt(items, "go", []);
    expect(next.map((i) => i.kind)).toEqual(["user", "assistant", "apikey"]);
    expect(next[1]).toMatchObject({ kind: "assistant", text: "One moment", streaming: false });
  });

  it("retires the key card and opens a fresh reply bubble on resolve", () => {
    const items = appendApiKeyPrompt(startTurn([], "hi"), "hi", []);
    const card = items.find((i) => i.kind === "apikey")!;
    const next = resolveApiKeyPrompt(items, card.id);
    expect(next.some((i) => i.kind === "apikey")).toBe(false);
    expect(next[next.length - 1]).toMatchObject({ kind: "assistant", text: "", streaming: true });
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

  it("extracts image attachments from a multimodal user message", () => {
    const items = transcriptToItems([
      {
        role: "user",
        content: [
          { type: "text", text: "What is in this image?" },
          { type: "image_url", image_url: { url: ".oxen-harness/attachments/abc.png" } },
        ],
      },
    ]);
    expect(items[0]).toMatchObject({
      kind: "user",
      text: "What is in this image?",
      images: [".oxen-harness/attachments/abc.png"],
    });
  });
});

describe("thread: startTurn attachments", () => {
  it("attaches only image paths to the user bubble", () => {
    const [user] = startTurn([], "look", ["/abs/fox.png", "/abs/notes.pdf", "/abs/a.JPG"]);
    expect(user).toMatchObject({ kind: "user", images: ["/abs/fox.png", "/abs/a.JPG"] });
  });

  it("leaves images undefined when there are no image attachments", () => {
    const [user] = startTurn([], "hi", ["/abs/notes.pdf"]);
    expect(user).toMatchObject({ kind: "user" });
    expect((user as { images?: string[] }).images).toBeUndefined();
  });
});
