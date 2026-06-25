import { useState } from "react";
import {
  Check,
  ChevronRight,
  FilePlus2,
  FileText,
  FolderSearch,
  GitBranch,
  Globe,
  KeyRound,
  type LucideIcon,
  MessageCircleQuestion,
  PencilLine,
  Search,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import { Button } from "../../components/ui";
import { configureBraveKey } from "../../lib/ipc";
import { basename, elapsed } from "../../lib/format";
import { canvasDocFromArgs } from "../../lib/canvas";
import { useStore } from "../../lib/store";
import type { Item } from "./thread";
import "./toolcall.css";

type ToolItem = Extract<Item, { kind: "tool" }>;

const MAX_BODY_CHARS = 4000;

/** Mirror of `harness_tools::web::WEB_SEARCH_NO_KEY` — the marker the web_search
 *  tool returns when no Brave key is set, so we can offer an inline key prompt. */
const WEB_SEARCH_NO_KEY = "Web search needs a Brave Search API key.";

/** A polished, tool-aware card for one tool call: an icon + human summary in the
 *  header, with the arguments/output revealed on click. Each tool gets a tailored
 *  body (a diff for edits, a terminal for shell, result cards for web search). */
export function ToolCall({ item, now }: { item: ToolItem; now: number }) {
  const [open, setOpen] = useState(false);
  const a = parseArgs(item.args);
  // A canvas call is a clickable card that (re)opens the document in the panel —
  // including past versions in a resumed chat, rebuilt from the call's args.
  if (item.name === "canvas") return <CanvasToolCall item={item} now={now} a={a} />;
  const { icon: Icon, verb, target } = present(item.name, a);
  const duration = toolDuration(item, now);

  // A web search that failed for a missing key gets a prominent, always-visible
  // key prompt rather than the generic (collapsed) output.
  const needsKey =
    item.name === "web_search" && !item.running && item.result.includes(WEB_SEARCH_NO_KEY);
  const failed = !item.running && item.result.startsWith("tool error:");
  const hasBody = !needsKey && Boolean(item.result || item.args);

  return (
    <div className={`toolcall ${item.running ? "running" : failed ? "failed" : "done"}`}>
      <button
        type="button"
        className="toolcall-head"
        onClick={() => hasBody && setOpen((o) => !o)}
        aria-expanded={hasBody ? open : undefined}
        disabled={!hasBody}
      >
        <span className="toolcall-icon">
          <Icon size={15} />
        </span>
        <span className="toolcall-summary">
          <span className="toolcall-verb">{verb}</span>
          {target && (
            <span className="toolcall-target" title={target}>
              {target}
            </span>
          )}
        </span>
        <span className="toolcall-meta">
          {duration && <span className="toolcall-time">{duration}</span>}
          {item.running ? (
            <span className="toolcall-spinner" aria-label="running" />
          ) : (
            <span className={`toolcall-dot ${failed ? "err" : ""}`} aria-hidden />
          )}
          {hasBody && <ChevronRight className={`toolcall-chevron ${open ? "open" : ""}`} size={15} />}
        </span>
      </button>

      {needsKey && (
        <div className="toolcall-body">
          <WebSearchKeyPrompt />
        </div>
      )}
      {open && hasBody && (
        <div className="toolcall-body">
          <ToolBody name={item.name} a={a} result={item.result} />
        </div>
      )}
    </div>
  );
}

/** A canvas tool call: a clickable card that opens (or reopens) the document in
 *  the side panel. Each call in the thread is a snapshot, so older cards reopen
 *  earlier versions — and they work in a resumed chat since the content is in
 *  the call's arguments. */
function CanvasToolCall({ item, now, a }: { item: ToolItem; now: number; a: Record<string, unknown> }) {
  const openCanvasDoc = useStore((s) => s.openCanvasDoc);
  const doc = canvasDocFromArgs(a);
  const title = typeof a.title === "string" && a.title.trim() ? (a.title as string) : "Document";
  const format = typeof a.format === "string" ? (a.format as string) : "markdown";
  const duration = toolDuration(item, now);
  const clickable = !item.running && !!doc;

  return (
    <div className={`toolcall canvas-tool ${item.running ? "running" : "done"}`}>
      <button
        type="button"
        className="toolcall-head"
        onClick={() => doc && openCanvasDoc(doc)}
        disabled={!clickable}
        title={clickable ? "Open in canvas" : undefined}
      >
        <span className="toolcall-icon">
          <FileText size={15} />
        </span>
        <span className="toolcall-summary">
          <span className="toolcall-verb">{item.running ? "Writing canvas" : "Canvas"}</span>
          <span className="toolcall-target" title={title}>
            {title}
          </span>
        </span>
        <span className="toolcall-meta">
          <span className="canvas-fmt">{format}</span>
          {duration && <span className="toolcall-time">{duration}</span>}
          {item.running ? (
            <span className="toolcall-spinner" aria-label="running" />
          ) : (
            <span className="canvas-open">open ›</span>
          )}
        </span>
      </button>
    </div>
  );
}

/** Inline prompt shown when web search ran without a Brave key: paste a key and
 *  it's applied to the live agent so the next search works in this same chat. */
function WebSearchKeyPrompt() {
  const [key, setKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    const k = key.trim();
    if (!k) return;
    setSaving(true);
    setError(null);
    try {
      await configureBraveKey(k);
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  if (saved) {
    return (
      <div className="ws-key saved">
        <Check size={16} />
        <span>Key saved — ask me to search again and it’ll work.</span>
      </div>
    );
  }

  return (
    <div className="ws-key">
      <div className="ws-key-head">
        <KeyRound size={16} />
        <span>Web search needs a Brave Search API key</span>
      </div>
      <p className="ws-key-sub">
        Get a free key at{" "}
        <a href="https://brave.com/search/api/" target="_blank" rel="noreferrer">
          brave.com/search/api
        </a>
        . It’s stored locally and enables the <code>web_search</code> tool.
      </p>
      <form
        className="ws-key-row"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <input
          className="field-input"
          type="password"
          placeholder="Paste your Brave Search API key"
          value={key}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          onChange={(e) => setKey(e.target.value)}
        />
        <Button type="submit" variant="primary" size="sm" disabled={saving || !key.trim()}>
          {saving ? "Saving…" : "Save key"}
        </Button>
      </form>
      {error && <div className="ws-key-error">{error}</div>}
    </div>
  );
}

// ---- per-tool header presentation ------------------------------------------

interface Presentation {
  icon: LucideIcon;
  verb: string;
  /** Emphasized subject (file path, command, query) — rendered monospace. */
  target?: string;
}

function present(name: string, a: Record<string, unknown>): Presentation {
  const s = (k: string) => (typeof a[k] === "string" ? (a[k] as string) : undefined);
  switch (name) {
    case "read_file":
      return { icon: FileText, verb: "Read", target: pathLabel(s("path")) };
    case "write_file":
      return { icon: FilePlus2, verb: "Wrote", target: pathLabel(s("path")) };
    case "edit_file":
      return { icon: PencilLine, verb: "Edited", target: pathLabel(s("path")) };
    case "find_files":
      return { icon: FolderSearch, verb: "Found files", target: s("pattern") };
    case "search_files":
      return { icon: Search, verb: "Searched for", target: s("pattern") };
    case "run_shell":
      return { icon: SquareTerminal, verb: "Ran", target: firstLine(s("command")) };
    case "git":
      return { icon: GitBranch, verb: "git", target: s("operation") };
    case "web_search":
      return { icon: Globe, verb: "Searched the web", target: s("query") };
    case "ask_user_question":
      return { icon: MessageCircleQuestion, verb: "Asked a question" };
    default:
      return { icon: Wrench, verb: prettyName(name) };
  }
}

// ---- per-tool body ---------------------------------------------------------

function ToolBody({ name, a, result }: { name: string; a: Record<string, unknown>; result: string }) {
  if (name === "edit_file" && typeof a.old_string === "string" && typeof a.new_string === "string") {
    return (
      <>
        <Diff oldText={a.old_string as string} newText={a.new_string as string} />
        {result && <Output text={result} />}
      </>
    );
  }

  if (name === "write_file" && typeof a.contents === "string") {
    return (
      <>
        <pre className="toolcall-code">{clamp(a.contents as string)}</pre>
        {result && <Output text={result} />}
      </>
    );
  }

  if (name === "run_shell") {
    return (
      <div className="toolcall-terminal">
        {typeof a.command === "string" && <div className="toolcall-cmd">$ {a.command as string}</div>}
        {result ? <pre className="toolcall-stdout">{clamp(result)}</pre> : <div className="toolcall-empty">No output</div>}
      </div>
    );
  }

  if (name === "web_search" && result) {
    const results = parseWebResults(result);
    if (results.length) {
      return (
        <div className="toolcall-results">
          {results.map((r, i) => (
            <a key={i} className="toolcall-result" href={r.url} target="_blank" rel="noreferrer">
              <span className="toolcall-result-title">{r.title}</span>
              <span className="toolcall-result-url">{r.url}</span>
              {r.snippet && <span className="toolcall-result-snippet">{r.snippet}</span>}
            </a>
          ))}
        </div>
      );
    }
  }

  return result ? <Output text={result} /> : <div className="toolcall-empty">No output</div>;
}

function Output({ text }: { text: string }) {
  return <pre className="toolcall-code">{clamp(text)}</pre>;
}

/** A compact block diff: removed lines (from `old`) then added lines (from `new`). */
function Diff({ oldText, newText }: { oldText: string; newText: string }) {
  return (
    <pre className="toolcall-diff">
      {clamp(oldText)
        .split("\n")
        .map((line, i) => (
          <div key={`o${i}`} className="diff-line del">
            <span className="diff-gutter">-</span>
            {line || " "}
          </div>
        ))}
      {clamp(newText)
        .split("\n")
        .map((line, i) => (
          <div key={`n${i}`} className="diff-line add">
            <span className="diff-gutter">+</span>
            {line || " "}
          </div>
        ))}
    </pre>
  );
}

// ---- helpers ---------------------------------------------------------------

function parseArgs(raw: string): Record<string, unknown> {
  if (!raw) return {};
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** Live elapsed time for a running call, frozen duration for a finished one, or
 *  null for calls restored from history (which carry no timing). */
function toolDuration(item: ToolItem, now: number): string | null {
  if (item.running) return elapsed(now - item.startedAt);
  if (item.startedAt) return elapsed((item.endedAt ?? item.startedAt) - item.startedAt);
  return null;
}

function pathLabel(path?: string): string | undefined {
  if (!path) return undefined;
  // Show the filename; the full path lives in the title attribute.
  return basename(path);
}

function firstLine(s?: string): string | undefined {
  return s?.split("\n")[0];
}

function prettyName(name: string): string {
  return name.replace(/_/g, " ");
}

function clamp(s: string): string {
  return s.length > MAX_BODY_CHARS ? s.slice(0, MAX_BODY_CHARS) + "\n…" : s;
}

interface WebResult {
  title: string;
  url: string;
  snippet: string;
}

/** Parse the Brave-search text the tool returns into structured result cards.
 *  Format: a header line, then numbered `N. title` / url / description blocks. */
function parseWebResults(raw: string): WebResult[] {
  const lines = raw.split("\n");
  const results: WebResult[] = [];
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^\s*\d+\.\s+(.*)$/);
    if (!m) continue;
    const title = m[1].trim();
    const url = (lines[i + 1] ?? "").trim();
    const snippet = (lines[i + 2] ?? "").trim();
    if (url.startsWith("http")) {
      results.push({ title, url, snippet });
      i += 2;
    }
  }
  return results;
}
