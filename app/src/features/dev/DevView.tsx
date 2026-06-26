// Developer view: inspect the raw inputs and outputs of every LLM call in the
// current session. Reads the session's persisted transcript verbatim (system
// prompt, user/assistant messages, tool calls, and tool results) and renders it
// either as a role-coded readable list or as raw JSON, with a summary header of
// usage and tool-call stats. Read-only — it never disturbs the live agent.

import { useCallback, useEffect, useMemo, useState } from "react";
import { Braces, Check, ChevronRight, Copy, RefreshCw, Wrench } from "lucide-react";
import { Modal } from "../../components/ui";
import { useStore } from "../../lib/store";
import { sessionMessages, toolDefinitions } from "../../lib/ipc";
import type { ChatMessage, MessageContent, ToolCall, ToolDefinition } from "../../lib/types";
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

export function DevView() {
  const setDevViewOpen = useStore((s) => s.setDevViewOpen);
  const session = useStore((s) => s.session);
  const running = useStore((s) => !!session && s.runStatus[session.session_id] === "running");

  const [messages, setMessages] = useState<ChatMessage[] | null>(null);
  const [tools, setTools] = useState<ToolDefinition[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<"pretty" | "raw">("pretty");

  const load = useCallback(async () => {
    if (!session) return;
    try {
      setError(null);
      const [msgs, defs] = await Promise.all([
        sessionMessages(session.session_id),
        toolDefinitions().catch(() => [] as ToolDefinition[]),
      ]);
      setMessages(msgs);
      setTools(defs);
    } catch (e) {
      setError(String(e));
    }
  }, [session]);

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
    session && session.context_window > 0
      ? Math.min(100, (session.context_tokens / session.context_window) * 100)
      : 0;

  // Tabs + actions live in the modal header bar (not floating over the body).
  const headerActions = session ? (
    <>
      <div className="dev-segmented" role="tablist">
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
      <button className="dev-copy dev-icon-only" onClick={load} title="Reload transcript" aria-label="Refresh">
        <RefreshCw size={13} />
      </button>
      {mode === "raw" && <CopyButton text={rawJson} label="Copy all" />}
    </>
  ) : undefined;

  return (
    <Modal title="Developer view" xwide actions={headerActions} onClose={() => setDevViewOpen(false)}>
      {!session ? (
        <p className="muted">No active session to inspect.</p>
      ) : (
        <div className="dev">
          {/* Summary header */}
          <div className="dev-summary">
            <Stat label="Model" value={session.model} mono />
            <Stat label="Messages" value={String(stats.count)} />
            <Stat label="LLM calls" value={String(stats.byRole.assistant ?? 0)} />
            <Stat
              label="Tools available"
              value={tools.length > 0 ? `${tools.length} · ~${toolTokens.toLocaleString()} tok` : "0"}
            />
            <Stat label="Tool calls" value={String(stats.toolCalls)} />
            <Stat label="Session tokens" value={session.tokens_used.toLocaleString()} />
            <Stat
              label="Context"
              value={`${session.context_tokens.toLocaleString()} / ${session.context_window.toLocaleString()} · ${
                ctxPct < 1 ? "<1" : Math.round(ctxPct)
              }%`}
            />
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
      )}
    </Modal>
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
