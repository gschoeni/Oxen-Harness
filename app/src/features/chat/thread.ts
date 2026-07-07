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
  | { id: string; kind: "notice"; text: string }
  // An inline API-key entry card, shown in place of a reply when a turn failed
  // authentication (a 401). It carries the failed prompt so the turn can be
  // retried once a key is saved.
  | { id: string; kind: "apikey"; text: string; attachments: string[] }
  // An inline "continue this chat" card, shown where the reply would be when a
  // turn died recoverably (e.g. a 402 out-of-credits error) or when a resumed
  // transcript ends mid-turn. `message` explains why; `text`/`attachments`
  // carry the failed prompt so a retry can fall back to the API-key card if it
  // then hits a 401.
  | { id: string; kind: "retry"; text: string; attachments: string[]; message: string }
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

/** Replace the in-flight assistant bubble with an inline API-key prompt after a
 *  turn failed authentication. Drops the empty streaming bubble (or settles a
 *  non-empty one), then appends a card carrying the failed turn so it can be
 *  retried in place once a key is entered. */
export function appendApiKeyPrompt(prev: Item[], text: string, attachments: string[]): Item[] {
  const next = [...prev];
  for (let i = next.length - 1; i >= 0; i--) {
    const it = next[i];
    if (it.kind === "assistant" && it.streaming) {
      if (it.text === "") next.splice(i, 1);
      else next[i] = { ...it, streaming: false };
      break;
    }
  }
  next.push({ id: uid(), kind: "apikey", text, attachments });
  return next;
}

/** Retire an inline recovery card (API-key prompt or retry card) once its
 *  action fired, and open a fresh streaming assistant bubble to receive the
 *  retried turn's reply. */
export function resolveRecoveryPrompt(prev: Item[], id: string): Item[] {
  const next = prev.filter((it) => it.id !== id);
  next.push({ id: uid(), kind: "assistant", text: "", streaming: true });
  return next;
}

/** Replace the in-flight assistant bubble with an inline retry card after a
 *  turn died recoverably (e.g. out of credits). Same shape as
 *  [`appendApiKeyPrompt`]: settle/drop the streaming bubble, then append a card
 *  carrying the failed turn so it can be continued in place. */
export function appendRetryPrompt(
  prev: Item[],
  text: string,
  attachments: string[],
  message: string,
): Item[] {
  const next = [...prev];
  for (let i = next.length - 1; i >= 0; i--) {
    const it = next[i];
    if (it.kind === "assistant" && it.streaming) {
      if (it.text === "") next.splice(i, 1);
      else next[i] = { ...it, streaming: false };
      break;
    }
  }
  next.push({ id: uid(), kind: "retry", text, attachments, message });
  return next;
}

/** Drop any pending retry cards — a fresh prompt supersedes them (the dangling
 *  user turn is still in the transcript, so the model answers both), and a
 *  stale card left behind would re-drive an already-settled transcript. */
export function dropRetryPrompts(prev: Item[]): Item[] {
  return prev.some((it) => it.kind === "retry") ? prev.filter((it) => it.kind !== "retry") : prev;
}

/** Whether a stored transcript stops mid-turn — it ends on a user message (the
 *  reply never arrived: an API error, out of credits, or the app closed) or on
 *  a tool result the model never got to react to. Such a chat can be continued
 *  in place by re-driving the transcript. */
export function endsMidTurn(messages: ChatMessage[]): boolean {
  const last = messages[messages.length - 1];
  return last !== undefined && (last.role === "user" || last.role === "tool");
}

/** The text of the transcript's last user message (for a retry card's carried
 *  prompt). Empty if there is none. */
export function lastUserText(messages: ChatMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") return contentText(messages[i].content);
  }
  return "";
}

/** Add a centered notice line (e.g. a context-compaction note). Inserts it
 *  before a trailing empty in-flight assistant bubble so the continued reply
 *  still streams below it. */
export function appendNotice(prev: Item[], text: string): Item[] {
  const notice: Item = { id: uid(), kind: "notice", text };
  const last = prev[prev.length - 1];
  if (last && last.kind === "assistant" && last.streaming && last.text === "") {
    return [...prev.slice(0, -1), notice, last];
  }
  return [...prev, notice];
}
