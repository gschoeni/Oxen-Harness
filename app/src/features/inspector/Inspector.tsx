// Transcript inspector: read the persisted messages of a chat verbatim (system
// prompt, user/assistant turns, tool calls, and tool results). Three renderings,
// increasingly deep: "Chat" (the same view as the main conversation — easiest to
// read), "Readable" (a role-coded list with per-message tokens + tool schemas),
// and "Raw JSON" (the exact request shape). Read-only — never disturbs the agent.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Ban,
  Braces,
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
  MessageSquare,
  RefreshCw,
  Wrench,
  X,
} from "lucide-react";
import { useStore } from "../../lib/store";
import { sessionMessages, toolDefinitions } from "../../lib/ipc";
import type { ChatMessage, MessageContent, ToolCall, ToolDefinition } from "../../lib/types";
import { ThreadItem } from "../chat/ThreadItem";
import { transcriptToItems } from "../chat/thread";
import "./dev.css";

const ROLE_LABEL: Record<string, string> = {
  system: "System prompt",
  user: "User",
  assistant: "Assistant",
  tool: "Tool result",
};

/** Pull the plain text out of a message's content (string or content parts). */
function textOf(content?: MessageContent): string {
  if (!content) return "";
  if (typeof content === "string") return content;
  return content
    .filter((p): p is { type: "text"; text: string } => p.type === "text")
    .map((p) => p.text)
    .join("\n");
}

/** Count non-text attachments (images / files) carried on a message. */
function attachmentsOf(content?: MessageContent): { images: number; files: number } {
  if (!content || typeof content === "string") return { images: 0, files: 0 };
  let images = 0;
  let files = 0;
  for (const p of content) {
    if (p.type === "image_url") images++;
    else if (p.type === "file") files++;
  }
  return { images, files };
}

/** Per-message token estimate, mirroring the backend's chars/4 + overhead. */
function estTokens(m: ChatMessage): number {
  let chars = m.role.length;
  const c = m.content;
  if (typeof c === "string") chars += c.length;
  else if (Array.isArray(c)) for (const p of c) if (p.type === "text") chars += p.text.length;
  for (const tc of m.tool_calls ?? [])
    chars += tc.function.name.length + tc.function.arguments.length + tc.id.length;
  if (m.tool_call_id) chars += m.tool_call_id.length;
  if (m.name) chars += m.name.length;
  return Math.round(chars / 4) + 4;
}

/** Token estimate for a JSON value, mirroring the backend's chars/4 heuristic
 *  for tool definitions (which add no per-item overhead). */
function estJsonTokens(value: unknown): number {
  return Math.floor(JSON.stringify(value).length / 4);
}

/** Pretty-print a JSON-string argument blob; fall back to the raw string. */
function prettyJson(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
}

function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="dev-copy"
      onClick={() => {
        navigator.clipboard.writeText(text).then(() => {
          setDone(true);
          setTimeout(() => setDone(false), 1200);
        });
      }}
      title={label}
    >
      {done ? <Check size={13} /> : <Copy size={13} />}
      {done ? "Copied" : label}
    </button>
  );
}

function ToolCallBlock({ call }: { call: ToolCall }) {
  return (
    <div className="dev-toolcall">
      <div className="dev-tool-head">
        <Wrench size={12} />
        <span className="dev-tool-name">{call.function.name}</span>
        <span className="dev-tool-id">{call.id}</span>
        <CopyButton text={prettyJson(call.function.arguments)} label="args" />
      </div>
      <pre className="dev-code">{prettyJson(call.function.arguments)}</pre>
    </div>
  );
}

/** One tool's definition: name, description, and collapsible JSON schema. */
function ToolDefRow({ tool }: { tool: ToolDefinition }) {
  const [open, setOpen] = useState(false);
  const name = tool.function?.name ?? "(unnamed)";
  const description = tool.function?.description ?? "";
  return (
    <div className="dev-toolcall">
      <button className="dev-tool-head dev-tooldef-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRight size={12} className={`dev-chevron ${open ? "open" : ""}`} />
        <Wrench size={12} />
        <span className="dev-tool-name">{name}</span>
        {description && <span className="dev-tooldef-desc">{description}</span>}
        <span className="dev-msg-tokens">~{estJsonTokens(tool).toLocaleString()} tok</span>
      </button>
      {open && <pre className="dev-code">{JSON.stringify(tool.function?.parameters ?? tool, null, 2)}</pre>}
    </div>
  );
}

/** The tool definitions advertised to the model — collapsed by default since
 *  they're reference schemas, not part of the conversation. */
function ToolsPanel({ tools }: { tools: ToolDefinition[] }) {
  const [open, setOpen] = useState(false);
  const totalTokens = tools.reduce((sum, t) => sum + estJsonTokens(t), 0);
  return (
    <div className="dev-msg dev-tools-panel">
      <button className="dev-msg-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRight size={14} className={`dev-chevron ${open ? "open" : ""}`} />
        <span className="dev-role">Tool definitions</span>
        <span className="dev-tag">{tools.length} available</span>
        {!open && (
          <span className="dev-preview">
            {tools.map((t) => t.function?.name).filter(Boolean).join(", ")}
          </span>
        )}
        <span className="dev-msg-tokens">~{totalTokens.toLocaleString()} tok</span>
      </button>
      {open && (
        <div className="dev-msg-body">
          {tools.map((t, i) => (
            <ToolDefRow key={t.function?.name ?? i} tool={t} />
          ))}
        </div>
      )}
    </div>
  );
}

function MessageCard({ message, index }: { message: ChatMessage; index: number }) {
  const role = message.role;
  // System prompts and tool results are long and rarely the focus, so collapse
  // them by default; user/assistant turns stay open.
  const [open, setOpen] = useState(role !== "system" && role !== "tool");
  const text = textOf(message.content);
  const { images, files } = attachmentsOf(message.content);
  const toolCalls = message.tool_calls ?? [];
  const preview = text.replace(/\s+/g, " ").slice(0, 80);

  return (
    <div className={`dev-msg dev-${role}`}>
      <button className="dev-msg-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRight size={14} className={`dev-chevron ${open ? "open" : ""}`} />
        <span className="dev-role">{ROLE_LABEL[role] ?? role}</span>
        <span className="dev-idx">#{index}</span>
        {message.name && <span className="dev-tag">{message.name}</span>}
        {toolCalls.length > 0 && (
          <span className="dev-tag">
            {toolCalls.length} tool call{toolCalls.length > 1 ? "s" : ""}
          </span>
        )}
        {!open && preview && <span className="dev-preview">{preview}</span>}
        <span className="dev-msg-tokens">~{estTokens(message).toLocaleString()} tok</span>
      </button>

      {open && (
        <div className="dev-msg-body">
          {text && <div className="dev-text">{text}</div>}
          {(images > 0 || files > 0) && (
            <div className="dev-attach">
              {images > 0 && <span>🖼 {images} image{images > 1 ? "s" : ""}</span>}
              {files > 0 && <span>📎 {files} file{files > 1 ? "s" : ""}</span>}
            </div>
          )}
          {toolCalls.map((tc) => (
            <ToolCallBlock key={tc.id} call={tc} />
          ))}
          {role === "tool" && message.tool_call_id && (
            <div className="dev-tool-ref">↳ result for {message.tool_call_id}</div>
          )}
          {text && (
            <div className="dev-msg-foot">
              <CopyButton text={text} label="Copy text" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** The raw LLM transcript inspector for a single chat (any session, not just the
 *  current one). Reads the persisted transcript verbatim (system prompt,
 *  user/assistant turns, tool calls + results) and renders it as a role-coded
 *  list or raw JSON, with a usage/tool summary header. Read-only. Rendered inside
 *  the {@link InspectorDrawer}. */
export function Inspector({ sessionId }: { sessionId: string }) {
  // Live session info (model, tokens, context) is only available for chats the
  // app currently holds; for a past chat we still show what we can derive from
  // the transcript itself. Tool definitions come from the *live* agent, so they
  // only make sense for the current chat.
  const info = useStore((s) => s.infos[sessionId] ?? (s.session?.session_id === sessionId ? s.session : undefined));
  const summaryModel = useStore((s) => s.sessions.find((x) => x.id === sessionId)?.model);
  const isCurrent = useStore((s) => s.session?.session_id === sessionId);
  const running = useStore((s) => s.runStatus[sessionId] === "running");
  const model = info?.model ?? summaryModel ?? "—";

  const [messages, setMessages] = useState<ChatMessage[] | null>(null);
  const [tools, setTools] = useState<ToolDefinition[]>([]);
  const [error, setError] = useState<string | null>(null);
  // "chat" is the default: the same rendering as the main chat view (easiest to
  // read). "pretty" and "raw" dig progressively deeper into the raw transcript.
  const [mode, setMode] = useState<"chat" | "pretty" | "raw">("chat");

  const load = useCallback(async () => {
    try {
      setError(null);
      setMessages(null);
      const [msgs, defs] = await Promise.all([
        sessionMessages(sessionId),
        isCurrent ? toolDefinitions().catch(() => [] as ToolDefinition[]) : Promise.resolve([]),
      ]);
      setMessages(msgs);
      setTools(defs);
    } catch (e) {
      setError(String(e));
    }
  }, [sessionId, isCurrent]);

  useEffect(() => {
    load();
  }, [load]);

  const stats = useMemo(() => {
    const msgs = messages ?? [];
    const byRole = { system: 0, user: 0, assistant: 0, tool: 0 } as Record<string, number>;
    const toolCounts: Record<string, number> = {};
    let toolCalls = 0;
    for (const m of msgs) {
      byRole[m.role] = (byRole[m.role] ?? 0) + 1;
      for (const tc of m.tool_calls ?? []) {
        toolCalls++;
        toolCounts[tc.function.name] = (toolCounts[tc.function.name] ?? 0) + 1;
      }
    }
    const topTools = Object.entries(toolCounts).sort((a, b) => b[1] - a[1]);
    return { count: msgs.length, byRole, toolCalls, topTools };
  }, [messages]);

  // The chat-view rendering reuses the main thread's items/components, so a
  // transcript reads exactly as it did in the conversation.
  const chatItems = useMemo(() => transcriptToItems(messages ?? []), [messages]);

  // Estimated tokens the tool definitions occupy in every request (chars/4,
  // matching the backend's budgeting heuristic).
  const toolTokens = useMemo(() => tools.reduce((sum, t) => sum + estJsonTokens(t), 0), [tools]);

  // The raw view shows the full request shape the model receives: the tool
  // definitions plus the transcript.
  const rawJson = useMemo(
    () => JSON.stringify({ tools, messages: messages ?? [] }, null, 2),
    [tools, messages],
  );

  const ctxPct =
    info && info.context_window > 0
      ? Math.min(100, (info.context_tokens / info.context_window) * 100)
      : 0;

  // A fixed "now" for relative timestamps in the chat view — this is a static
  // historical transcript, so it needn't tick.
  const now = Date.now();

  return (
    <div className="dev">
          {/* Toolbar — view toggle + transcript actions */}
          <div className="dev-toolbar">
            <div className="dev-segmented" role="tablist">
              <button
                role="tab"
                aria-selected={mode === "chat"}
                className={mode === "chat" ? "active" : ""}
                onClick={() => setMode("chat")}
              >
                <MessageSquare size={13} /> Chat
              </button>
              <button
                role="tab"
                aria-selected={mode === "pretty"}
                className={mode === "pretty" ? "active" : ""}
                onClick={() => setMode("pretty")}
              >
                Readable
              </button>
              <button
                role="tab"
                aria-selected={mode === "raw"}
                className={mode === "raw" ? "active" : ""}
                onClick={() => setMode("raw")}
              >
                <Braces size={13} /> Raw JSON
              </button>
            </div>
            <button
              className="dev-copy dev-icon-only"
              onClick={load}
              title="Reload transcript"
              aria-label="Refresh"
            >
              <RefreshCw size={13} />
            </button>
            {mode === "raw" && <CopyButton text={rawJson} label="Copy all" />}
          </div>

          {/* Summary header */}
          <div className="dev-summary">
            <Stat label="Model" value={model} mono />
            <Stat label="Messages" value={String(stats.count)} />
            <Stat label="LLM calls" value={String(stats.byRole.assistant ?? 0)} />
            {isCurrent && (
              <Stat
                label="Tools available"
                value={tools.length > 0 ? `${tools.length} · ~${toolTokens.toLocaleString()} tok` : "0"}
              />
            )}
            <Stat label="Tool calls" value={String(stats.toolCalls)} />
            {info && (
              <>
                <Stat label="Session tokens" value={info.tokens_used.toLocaleString()} />
                <Stat
                  label="Context"
                  value={`${info.context_tokens.toLocaleString()} / ${info.context_window.toLocaleString()} · ${
                    ctxPct < 1 ? "<1" : Math.round(ctxPct)
                  }%`}
                />
              </>
            )}
          </div>

          {stats.topTools.length > 0 && (
            <div className="dev-tools-row">
              <span className="dev-tools-label">Tools used:</span>
              {stats.topTools.map(([name, n]) => (
                <span className="dev-tool-chip" key={name}>
                  {name} <b>{n}</b>
                </span>
              ))}
            </div>
          )}

          {running && (
            <span className="dev-live">● live — turn in progress; refresh for the latest</span>
          )}

          {/* Body */}
          {error ? (
            <p className="dev-error">⚠ {error}</p>
          ) : messages === null ? (
            <p className="muted">Loading transcript…</p>
          ) : messages.length === 0 ? (
            <p className="muted">This session has no messages yet.</p>
          ) : mode === "chat" ? (
            <div className="dev-chatview">
              <div className="thread">
                {chatItems.map((it) => (
                  <ThreadItem key={it.id} item={it} now={now} />
                ))}
              </div>
            </div>
          ) : mode === "raw" ? (
            <pre className="dev-rawjson">{rawJson}</pre>
          ) : (
            <div className="dev-messages">
              {tools.length > 0 && <ToolsPanel tools={tools} />}
              {messages.map((m, i) => (
                <MessageCard key={i} message={m} index={i} />
              ))}
            </div>
          )}
    </div>
  );
}

/** The right-side drawer that hosts the {@link Inspector}. Opened either to
 *  plainly read a chat (the chat's </> button → `openInspector`) or to review a
 *  queue of chats for the training dataset (the dataset builder → `openReview`),
 *  in which case it adds Keep/Reject controls that auto-advance to the next chat
 *  and prev/next navigation through the queue. */
export function InspectorDrawer() {
  const inspector = useStore((s) => s.inspector);
  const close = useStore((s) => s.closeInspector);
  const step = useStore((s) => s.reviewStep);
  const setStatus = useStore((s) => s.setReviewStatus);
  const summary = useStore((s) =>
    inspector ? s.sessions.find((x) => x.id === inspector.sessionId) : undefined,
  );

  useEffect(() => {
    if (!inspector) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      else if (inspector.review && e.key === "ArrowLeft") step(-1);
      else if (inspector.review && e.key === "ArrowRight") step(1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [inspector, close, step]);

  if (!inspector) return null;
  const { sessionId, review } = inspector;
  const title = summary?.title ?? "Chat transcript";
  const status = summary?.review_status ?? "";

  async function decide(next: "kept" | "rejected") {
    // Toggling the current status back to unreviewed feels natural on re-click.
    await setStatus(sessionId, status === next ? "" : next);
    if (review) step(1);
  }

  return (
    <div className="inspector-scrim" onClick={close}>
      <aside
        className="inspector-drawer"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="Chat transcript"
      >
        <header className="inspector-head">
          <div className="inspector-title-wrap">
            <span className="inspector-title" title={title}>
              {title}
            </span>
            {review && (
              <span className="inspector-progress">
                {review.index + 1} / {review.queue.length}
              </span>
            )}
          </div>
          <button className="inspector-close" onClick={close} aria-label="Close">
            <X size={18} />
          </button>
        </header>

        {review && (
          <div className="inspector-review-bar">
            <button
              className="inspector-nav"
              onClick={() => step(-1)}
              disabled={review.index === 0}
              aria-label="Previous chat"
            >
              <ChevronLeft size={16} />
            </button>
            <div className="inspector-decide">
              <button
                className={`inspector-keep ${status === "kept" ? "active" : ""}`}
                onClick={() => decide("kept")}
              >
                <Check size={15} /> Keep
              </button>
              <button
                className={`inspector-reject ${status === "rejected" ? "active" : ""}`}
                onClick={() => decide("rejected")}
              >
                <Ban size={15} /> Reject
              </button>
            </div>
            <button
              className="inspector-nav"
              onClick={() => step(1)}
              disabled={review.index === review.queue.length - 1}
              aria-label="Next chat"
            >
              <ChevronRight size={16} />
            </button>
          </div>
        )}

        <div className="inspector-body">
          {/* Remount on session change so the transcript reloads while paging. */}
          <Inspector key={sessionId} sessionId={sessionId} />
        </div>
      </aside>
    </div>
  );
}

function Stat({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="dev-stat">
      <span className="dev-stat-label">{label}</span>
      <span className={`dev-stat-value ${mono ? "mono" : ""}`}>{value}</span>
    </div>
  );
}
