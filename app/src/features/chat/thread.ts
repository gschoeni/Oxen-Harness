// Pure state model for the chat thread. The Chat component holds an `Item[]`
// and applies these transforms as the agent streams; keeping them here (free of
// React) makes the streaming behavior easy to read, reuse, and unit-test.

import type { ChatMessage, MessageContent } from "../../lib/types";
import { isImagePath } from "../../lib/attachments";

/** The displayable text of a message's content. A plain string passes through;
 *  a multimodal Parts array (from an attachment message) contributes only its
 *  text parts — without this, an array would render as "[object Object]". */
function contentText(content: MessageContent | null | undefined): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    // ContentPart is a discriminated union; narrow to text parts in the closure
    // so we only read `.text` where it exists.
    return content.flatMap((p) => (p.type === "text" ? [p.text] : [])).join("");
  }
  return "";
}

/** Image attachment references on a message (the stored relative path or URL). */
function imageRefs(content: MessageContent | null | undefined): string[] {
  if (Array.isArray(content)) {
    return content.flatMap((p) => (p.type === "image_url" ? [p.image_url.url] : []));
  }
  return [];
}

export type Item =
  | { id: string; kind: "user"; text: string; images?: string[] }
  | { id: string; kind: "assistant"; text: string; streaming: boolean; error?: boolean }
  | {
      id: string;
      kind: "tool";
      name: string;
      /** Raw JSON arguments the model called the tool with. */
      args: string;
      /** The tool's output (filled in when it finishes). */
      result: string;
      running: boolean;
      startedAt: number;
      endedAt?: number;
    };

let counter = 0;
/** A stable, unique React key for a thread item. */
export const uid = () => `i${counter++}`;

/** Re-render a stored transcript into thread items. System prompts and raw tool
 *  results are omitted; an assistant's tool calls become finished tool chips. */
export function transcriptToItems(messages: ChatMessage[]): Item[] {
  // Tool results arrive as separate `tool`-role messages keyed by call id; index
  // them first so each tool call can be shown with its output.
  const results = new Map<string, string>();
  for (const m of messages) {
    if (m.role === "tool" && m.tool_call_id) results.set(m.tool_call_id, contentText(m.content));
  }

  const items: Item[] = [];
  for (const m of messages) {
    if (m.role === "user") {
      const text = contentText(m.content);
      const images = imageRefs(m.content);
      if (text || images.length)
        items.push({ id: uid(), kind: "user", text, images: images.length ? images : undefined });
    } else if (m.role === "assistant") {
      const text = contentText(m.content);
      if (text) items.push({ id: uid(), kind: "assistant", text, streaming: false });
      for (const tc of m.tool_calls ?? []) {
        items.push({
          id: uid(),
          kind: "tool",
          name: tc.function?.name || "tool",
          args: tc.function?.arguments || "",
          result: (tc.id && results.get(tc.id)) || "",
          running: false,
          startedAt: 0,
        });
      }
    }
  }
  return items;
}

/** The user's prompt (with any image attachments) plus an empty in-flight
 *  assistant bubble for its reply. */
export function startTurn(prev: Item[], prompt: string, attachments: string[] = []): Item[] {
  const images = attachments.filter(isImagePath);
  return [
    ...prev,
    { id: uid(), kind: "user", text: prompt, images: images.length ? images : undefined },
    { id: uid(), kind: "assistant", text: "", streaming: true },
  ];
}

/** Append a streamed token to the in-flight assistant bubble (creating one if
 *  the previous bubble was retired by a tool call). */
export function appendToken(prev: Item[], token: string): Item[] {
  const next = [...prev];
  for (let i = next.length - 1; i >= 0; i--) {
    const it = next[i];
    if (it.kind === "assistant" && it.streaming) {
      next[i] = { ...it, text: it.text + token };
      return next;
    }
  }
  next.push({ id: uid(), kind: "assistant", text: token, streaming: true });
  return next;
}

/** Begin a tool chip with the call's arguments. Retire an empty "thinking"
 *  bubble it supersedes, or finalize a non-empty preamble bubble (e.g. "I'll
 *  build that…") so its activity indicator hands off to the tool chip. */
export function toolStart(prev: Item[], name: string, args: string, startedAt: number): Item[] {
  let next = [...prev];
  const last = next[next.length - 1];
  if (last && last.kind === "assistant" && last.streaming) {
    if (last.text === "") {
      next = next.slice(0, -1);
    } else {
      next[next.length - 1] = { ...last, streaming: false };
    }
  }
  next.push({ id: uid(), kind: "tool", name, args, result: "", running: true, startedAt });
  return next;
}

/** Finish the matching running tool chip (recording its result) and open a
 *  fresh bubble for any text the model emits next. */
export function toolEnd(prev: Item[], name: string, result: string, endedAt: number): Item[] {
  const next = [...prev];
  for (let i = next.length - 1; i >= 0; i--) {
    const it = next[i];
    if (it.kind === "tool" && it.running && it.name === name) {
      next[i] = { ...it, running: false, result: result || it.result, endedAt };
      break;
    }
  }
  next.push({ id: uid(), kind: "assistant", text: "", streaming: true });
  return next;
}

/** Settle the in-flight assistant bubble: keep streamed text, fall back to the
 *  returned `final`, and drop the bubble if it ended up empty. */
export function finalizeAssistant(prev: Item[], final: string, error = false): Item[] {
  const next = [...prev];
  for (let i = next.length - 1; i >= 0; i--) {
    const it = next[i];
    if (it.kind === "assistant" && it.streaming) {
      const text = it.text || final || "";
      if (text === "") next.splice(i, 1);
      else next[i] = { ...it, text, streaming: false, error };
      return next;
    }
  }
  if (final) next.push({ id: uid(), kind: "assistant", text: final, streaming: false, error });
  return next;
}
