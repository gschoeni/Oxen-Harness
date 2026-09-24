import { describe, expect, it } from "vitest";
import {
  appendApiKeyPrompt,
  appendNotice,
  appendRetryPrompt,
  appendToken,
  capThread,
  dropRetryPrompts,
  endsMidTurn,
  finalizeAssistant,
  lastModelError,
  lastUserText,
  resolveRecoveryPrompt,
  resumeMidTurn,
  startTurn,
  toolEnd,
  toolStart,
  transcriptToItems,
  type Item,
} from "./thread";
import type { ModelErrorDetail } from "../../lib/types";

const sampleModelError = (over: Partial<ModelErrorDetail> = {}): ModelErrorDetail => ({
  at: 1_700_000_000_000,
  error: "Oxen API error (502): The model provider returned an error.",
  model: "claude-opus-4-8",
  endpoint: "https://hub.oxen.ai/api/ai",
  status: 502,
  attempt: 1,
  maxAttempts: 4,
  next: { kind: "retry", delayMs: 1000 },
  ...over,
});

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

  it("ends a bubble that has text, so the next round's reply starts its own", () => {
    const items = appendToken(startTurn([], "hi"), "Queued — I'll show it when it lands.");
    const noticed = appendNotice(items, "nudged the model: the reply announced work without doing it");
    const next = appendToken(noticed, "Here's the proof.");
    expect(next.map((i) => i.kind)).toEqual(["user", "assistant", "notice", "assistant"]);
    expect(assistantText(next).map((a) => a.text)).toEqual(["Queued — I'll show it when it lands.", "Here's the proof."]);
  });

  it("appends at the end when there is no in-flight bubble", () => {
    const items = finalizeAssistant(startTurn([], "hi"), "done");
    const next = appendNotice(items, "note");
    expect(next[next.length - 1]).toMatchObject({ kind: "notice", text: "note" });
    // A plain note carries no error payload at all (not even an undefined key).
    expect("error" in next[next.length - 1]).toBe(false);
  });

  it("keeps a failed model call's structured detail on the notice", () => {
    const error = sampleModelError({ status: 502 });
    const next = appendNotice(startTurn([], "hi"), "Model call failed (502) — retrying in 1s", error);
    expect(next[1]).toMatchObject({ kind: "notice", error: { status: 502, model: "claude-opus-4-8" } });
  });
});

describe("thread: lastModelError", () => {
  it("finds the last failed call of the current turn only", () => {
    const first = sampleModelError({ status: 502, attempt: 1 });
    const second = sampleModelError({ status: 503, attempt: 2 });
    let items = appendNotice(startTurn([], "one"), "failed", first);
    items = finalizeAssistant(items, "recovered");
    // A new turn with no failures yet: the previous turn's hiccup is not "its" error.
    items = startTurn(items, "two");
    expect(lastModelError(items)).toBeUndefined();
    items = appendNotice(items, "failed again", first);
    items = appendNotice(items, "and again", second);
    expect(lastModelError(items)).toMatchObject({ status: 503, attempt: 2 });
  });

  it("attaches that error to the retry card when the turn dies", () => {
    const error = sampleModelError({ status: 502, detail: '{"error":"upstream"}' });
    const items = appendNotice(startTurn([], "hi"), "failed", error);
    const card = appendRetryPrompt(items, "hi", [], "the model endpoint failed 4 times in a row");
    expect(card[card.length - 1]).toMatchObject({
      kind: "retry",
      message: "the model endpoint failed 4 times in a row",
      lastError: { status: 502, detail: '{"error":"upstream"}' },
    });
    // No failure notice this turn (a 402 dies on the first call): no payload.
    const bare = appendRetryPrompt(startTurn([], "hi"), "hi", [], "out of credits");
    expect("lastError" in bare[bare.length - 1]).toBe(false);
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
    const next = resolveRecoveryPrompt(items, card.id);
    expect(next.some((i) => i.kind === "apikey")).toBe(false);
    expect(next[next.length - 1]).toMatchObject({ kind: "assistant", text: "", streaming: true });
  });
});

describe("thread: retry prompt", () => {
  it("swaps the empty reply bubble for a retry card carrying the failed turn", () => {
    const items = startTurn([], "build it"); // [user, empty assistant]
    const next = appendRetryPrompt(items, "build it", [], "Oxen API error (402): out of credits");
    expect(next.map((i) => i.kind)).toEqual(["user", "retry"]); // empty bubble dropped
    expect(next[1]).toMatchObject({
      kind: "retry",
      text: "build it",
      message: "Oxen API error (402): out of credits",
    });
  });

  it("appends a card to a settled thread (a resumed broken transcript)", () => {
    const items = transcriptToItems([{ role: "user", content: "hello" }]);
    const next = appendRetryPrompt(items, "hello", [], "stopped mid-turn");
    expect(next.map((i) => i.kind)).toEqual(["user", "retry"]);
  });

  it("retires the retry card and opens a fresh reply bubble on resolve", () => {
    const items = appendRetryPrompt(startTurn([], "hi"), "hi", [], "boom");
    const card = items.find((i) => i.kind === "retry")!;
    const next = resolveRecoveryPrompt(items, card.id);
    expect(next.some((i) => i.kind === "retry")).toBe(false);
    expect(next[next.length - 1]).toMatchObject({ kind: "assistant", text: "", streaming: true });
  });

  it("drops pending retry cards when a fresh prompt supersedes them", () => {
    const items = appendRetryPrompt(startTurn([], "hi"), "hi", [], "boom");
    expect(dropRetryPrompts(items).some((i) => i.kind === "retry")).toBe(false);
    // No cards → the same array back (no pointless re-render).
    const settled = finalizeAssistant(startTurn([], "q"), "done");
    expect(dropRetryPrompts(settled)).toBe(settled);
  });
});

describe("thread: broken-transcript detection", () => {
  it("flags a transcript ending on a user message or dangling tool result", () => {
    expect(endsMidTurn([{ role: "user", content: "hi" }])).toBe(true);
    expect(
      endsMidTurn([
        { role: "user", content: "go" },
        { role: "assistant", content: "", tool_calls: [] },
        { role: "tool", content: "output", tool_call_id: "1" },
      ]),
    ).toBe(true);
  });

  it("does not flag an empty or completed transcript", () => {
    expect(endsMidTurn([])).toBe(false);
    expect(
      endsMidTurn([
        { role: "user", content: "hi" },
        { role: "assistant", content: "hello!" },
      ]),
    ).toBe(false);
  });

  it("finds the last user message's text", () => {
    expect(
      lastUserText([
        { role: "user", content: "first" },
        { role: "assistant", content: "ok" },
        { role: "user", content: "second" },
      ]),
    ).toBe("second");
    expect(lastUserText([])).toBe("");
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

describe("thread: resident memory bounds", () => {
  it("releases old display items while preserving the newest conversation tail", () => {
    const items: Item[] = Array.from({ length: 350 }, (_, i) => ({
      id: `u${i}`,
      kind: "user" as const,
      text: String(i),
    }));
    const bounded = capThread(items);
    expect(bounded).toHaveLength(300);
    expect(bounded[0]).toMatchObject({ kind: "notice" });
    expect(bounded[bounded.length - 1]).toMatchObject({ text: "349" });
  });

  it("truncates large tool payloads in the display model", () => {
    const items = toolEnd(
      toolStart([], "shell", "a".repeat(20_000), 1),
      "shell",
      "r".repeat(8_000),
      2,
    );
    const tool = items[0] as Extract<Item, { kind: "tool" }>;
    expect(tool.args.length).toBeLessThan(20_000);
    expect(tool.result.length).toBeLessThan(8_000);
  });
});

describe("concurrent tool calls", () => {
  it("pairs each end with its start by call id, in any order", () => {
    let items: Item[] = [];
    items = toolStart(items, "read_file", '{"path":"a"}', 1, "call_a");
    items = toolStart(items, "read_file", '{"path":"b"}', 2, "call_b");
    items = toolEnd(items, "read_file", "B", 3, "call_b");
    items = toolEnd(items, "read_file", "A", 4, "call_a");
    const tools = items.filter((i) => i.kind === "tool");
    expect(tools.map((t) => t.kind === "tool" && [t.callId, t.running, t.result])).toEqual([
      ["call_a", false, "A"],
      ["call_b", false, "B"],
    ]);
    // Two ends opened one fresh bubble, not two.
    expect(items.filter((i) => i.kind === "assistant").length).toBe(1);
  });

  it("falls back to the newest running chip of that name without ids", () => {
    let items: Item[] = [];
    items = toolStart(items, "read_file", "", 1);
    items = toolEnd(items, "read_file", "done", 2);
    const tool = items.find((i) => i.kind === "tool");
    expect(tool && tool.kind === "tool" && tool.result).toBe("done");
  });
});

describe("thread: rejoining a reply mid-stream", () => {
  it("resumeMidTurn opens a partial bubble that the final text replaces in full", () => {
    // The transcript so far, then a bubble for the tail of a reply whose
    // head streamed while the thread was out of memory.
    let items = resumeMidTurn(transcriptToItems([{ role: "user", content: "hi" }]));
    expect(items[items.length - 1]).toMatchObject({ kind: "assistant", streaming: true, partial: true });
    items = appendToken(items, " world");
    // Only a tail was seen — the returned final is the whole reply, so it wins.
    items = finalizeAssistant(items, "hello world");
    expect(assistantText(items)[0]).toMatchObject({ text: "hello world", streaming: false });
    expect(assistantText(items)[0].partial).toBeUndefined();
  });

  it("a partial bubble keeps its streamed tail when the turn returns nothing", () => {
    const items = finalizeAssistant(appendToken(resumeMidTurn([]), "tail"), "");
    expect(assistantText(items)[0].text).toBe("tail");
  });

  it("a normal bubble still prefers what it streamed over the final", () => {
    const items = finalizeAssistant(appendToken(startTurn([], "q"), "streamed"), "final");
    expect(assistantText(items)[0].text).toBe("streamed");
  });
});

describe("thread: transcriptToItems hides the agent's tool-image hand-off", () => {
  it("drops the synthetic user bubble but keeps the tool card and reply", () => {
    const items = transcriptToItems([
      { role: "user", content: "make an ox" },
      {
        role: "assistant",
        content: "",
        tool_calls: [
          { id: "c1", type: "function", function: { name: "generate_image", arguments: '{"prompt":"an ox"}' } },
        ],
      },
      { role: "tool", tool_call_id: "c1", content: "Generated 1 image with flux:\n- generations/a.png (64×36)" },
      {
        role: "user",
        content: [
          { type: "text", text: "The image(s) produced by the tool call above:" },
          { type: "image_url", image_url: { url: "/abs/generations/a.png" } },
        ],
      },
      { role: "assistant", content: "Here it is." },
    ]);
    expect(items.map((i) => i.kind)).toEqual(["user", "tool", "assistant"]);
    expect(items.filter((i) => i.kind === "user").map((i) => (i as { text: string }).text)).toEqual(["make an ox"]);
  });
});
