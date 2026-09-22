import { useEffect, useState, type DragEvent } from "react";
import {
  Check,
  ChevronRight,
  Clapperboard,
  Images,
  ListVideo,
  Sparkles,
  FilePlus2,
  FileText,
  FolderSearch,
  GitBranch,
  Globe,
  KeyRound,
  ListChecks,
  type LucideIcon,
  MessageCircleQuestion,
  PencilLine,
  Search,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import { Button } from "../../components/ui";
import { configureBraveKey } from "../../lib/ipc";
import { elapsed, relPath } from "../../lib/format";
import { canvasDocFromArgs } from "../../lib/canvas";
import { planItemsFromArgs, planProgress } from "../../lib/plan";
import {
  MEDIA_MODELS_TOOL,
  MEDIA_NO_KEY,
  MEDIA_STATUS_TOOL,
  aspectFor,
  generatedFiles,
  generationArgs,
  isGenerateTool, isActiveUpload, uploadPercent } from "../../lib/media";
import { fleetsFor, useStore } from "../../lib/store";
import { agentRows, isActive } from "./agentRows";
import { AgentRowList, StopFleets, agentIdsInResult } from "./AgentRowList";
import { compactTokens, formatUsd } from "../../lib/format";
import { setDragPaths } from "../files/dnd";
import { UploadRow } from "./MediaPanel";
import { useAssetSrc } from "../files/useAssetSrc";
import type { Item } from "./thread";
import { PlanChecklist } from "./Plan";
import "./toolcall.css";
import "./plan.css";

type ToolItem = Extract<Item, { kind: "tool" }>;

const MAX_BODY_CHARS = 4000;

/** Mirror of `harness_tools::web::WEB_SEARCH_NO_KEY` — the marker the web_search
 *  tool returns when no Brave key is set, so we can offer an inline key prompt. */
const WEB_SEARCH_NO_KEY = "Web search needs a Brave Search API key.";

/** Ticks once a second while `active`, driving a running call's elapsed label.
 *  Local to each card so a live timer never re-renders the rest of the thread. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/** The tools whose card carries the lanes it started. */
const FLEET_TOOLS = new Set(["spawn_agents", "map_agents", "send_to_agent"]);

/** The lanes a spawn call started, shown inside its card: live from the
 *  fleet the call id ties to it while it runs, and from the saved records the
 *  result names ("agent id: …") for good — so a resumed chat still shows what
 *  each spawn did, where it did it. */
function FleetLanes({ item }: { item: ToolItem }) {
  const session = useStore((s) => s.session?.session_id ?? "");
  const fleets = useStore((s) => s.fleets);
  const agents = useStore((s) => s.agents[session]);
  const refresh = useStore((s) => s.refreshAgents);
  const mine = fleetsFor(fleets, session).filter(([, f]) => Boolean(item.callId) && f.call === item.callId);
  const ids = new Set<string>(agentIdsInResult(item.result));
  for (const [, f] of mine) for (const lane of f.lanes) if (lane.id) ids.add(lane.id);
  // Descendants of this call's lanes belong to the card too.
  const saved = agents ?? [];
  let grew = true;
  while (grew) {
    grew = false;
    for (const a of saved) {
      if (!ids.has(a.id) && a.parent && ids.has(a.parent)) {
        ids.add(a.id);
        grew = true;
      }
    }
  }
  const named = agentIdsInResult(item.result);
  const known = named.length > 0 && saved.some((a) => ids.has(a.id));
  useEffect(() => {
    if (session && named.length > 0 && !known) void refresh(session);
    // The result names the lanes once; that string is the dependency.
  }, [session, item.result, known, refresh]); // eslint-disable-line react-hooks/exhaustive-deps
  const rows = agentRows(
    saved.filter((a) => ids.has(a.id)),
    mine,
  );
  const spend = useStore((s) => s.treeUsage[session]);
  if (!rows.length) return null;
  const working = rows.filter((r) => isActive(r.status)).length;
  const running = mine.filter(([, f]) => !f.finished);
  const budget = running.slice(-1)[0]?.[1].budget;
  return (
    <div className="toolcall-agents">
      {working > 0 && (
        <div className="toolcall-agents-bar">
          <span>{working} working</span>
          {budget && (
            <span title={`${compactTokens(budget.tokens)} of ${compactTokens(budget.max_tokens)} shared tokens`}>
              {compactTokens(budget.tokens)} tokens
            </span>
          )}
          {spend?.cost != null && (
            <span title="This chat's spend so far, agents included">{formatUsd(spend.cost)}</span>
          )}
          <span className="toolcall-agents-spacer" />
          <StopFleets session={session} fleets={running} />
        </div>
      )}
      <AgentRowList session={session} rows={rows} />
    </div>
  );
}

/** A polished, tool-aware card for one tool call: an icon + human summary in the
 *  header, with the arguments/output revealed on click. Each tool gets a tailored
 *  body (a diff for edits, a terminal for shell, result cards for web search). */
export function ToolCall({ item }: { item: ToolItem }) {
  const [open, setOpen] = useState(false);
  const root = useStore((s) => s.session?.workspace ?? null);
  const now = useNow(item.running);
  const a = parseArgs(item.args);
  const failed = !item.running && item.result.startsWith("tool error:");
  // A canvas call is a clickable card that (re)opens the document in the panel —
  // including past versions in a resumed chat, rebuilt from the call's args.
  // One the tool refused (no arguments, a bad format) has no document to
  // open and reads as the error it was.
  if (item.name === "canvas" && !failed) return <CanvasToolCall item={item} a={a} />;
  // A plan update renders as a checklist snapshot (the live, pinned plan lives
  // above the thread; this card is the in-thread record of each update).
  if (item.name === "update_plan") return <PlanToolCard item={item} a={a} />;
  // A generation shows its prompt and, once done, the pictures themselves.
  if (isGenerateTool(item.name)) return <MediaToolCall item={item} a={a} />;
  const { icon: Icon, verb, target } = present(item.name, a, root);
  const duration = toolDuration(item, now);

  // A web search that failed for a missing key gets a prominent, always-visible
  // key prompt rather than the generic (collapsed) output.
  const needsKey =
    item.name === "web_search" && !item.running && item.result.includes(WEB_SEARCH_NO_KEY);
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
          {hasBody && <ChevronRight className={`chevron ${open ? "open" : ""}`} size={15} />}
        </span>
      </button>

      {FLEET_TOOLS.has(item.name) && <FleetLanes item={item} />}
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
function CanvasToolCall({ item, a }: { item: ToolItem; a: Record<string, unknown> }) {
  const openCanvasDoc = useStore((s) => s.openCanvasDoc);
  const now = useNow(item.running);
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

/** An image/video generation: the prompt, the model, reference chips, and a
 *  tile per output — skeletons while it renders, the saved files when done
 *  (parsed from the result, so it works in a resumed chat too). Clicking a
 *  tile opens the Gallery. */
function MediaToolCall({ item, a }: { item: ToolItem; a: Record<string, unknown> }) {
  const [expanded, setExpanded] = useState(false);
  const workspace = useStore((s) => s.session?.workspace ?? null);
  const library = useStore((s) => (s.session?.workspace ? s.media[s.session.workspace] : undefined));
  const sessionId = useStore((s) => s.session?.session_id);
  const uploads = useStore((s) => (s.session?.workspace ? s.mediaUploads[s.session.workspace] : undefined));
  const openGallery = useStore((s) => s.openGallery);
  const openInViewer = useStore((s) => s.openInViewer);
  const openSettings = useStore((s) => s.openSettings);
  const mediaPrefs = useStore((s) => s.mediaPrefs);
  const ensureMediaPrefs = useStore((s) => s.ensureMediaPrefs);
  useEffect(() => ensureMediaPrefs(), [ensureMediaPrefs]);
  const now = useNow(item.running);
  const g = generationArgs(item.name, a);
  // A call that named no model ran on the user's default for its kind.
  const model =
    g.model ?? (g.kind === "video" ? mediaPrefs?.default_video_model : mediaPrefs?.default_image_model) ?? "default model";
  const duration = toolDuration(item, now);
  const failed = !item.running && item.result.startsWith("tool error:");
  const needsKey = !item.running && item.result.includes(MEDIA_NO_KEY);
  const files = item.running ? [] : generatedFiles(item.result);
  const Icon = g.kind === "video" ? Clapperboard : Sparkles;
  const noun = g.kind === "video" ? "video" : "image";
  const verb = item.running
    ? `Generating ${g.count > 1 ? `${g.count} ${noun}s` : noun}`
    : files.length
      ? `Generated ${files.length > 1 ? `${files.length} ${noun}s` : noun}`
      : failed
        ? `${noun[0].toUpperCase()}${noun.slice(1)} generation failed`
        : `${noun[0].toUpperCase()}${noun.slice(1)} generation`;
  const detail = [model, g.duration ? `${g.duration}s` : null, g.resolution, g.aspectRatio]
    .filter(Boolean)
    .join(" · ");
  const long = g.prompt.length > 280;
  const prompt = expanded || !long ? g.prompt : `${g.prompt.slice(0, 280)}…`;
  // References on their way to the hub before the generation starts: the
  // card shows their bars in place of the rendering skeleton.
  const mine = item.running ? (uploads ?? []).filter((u) => u.session === sessionId) : [];
  const pending = mine.filter(isActiveUpload);
  const settled = mine.length - pending.filter((u) => u.status !== "failed").length;
  const overallPercent = mine.length
    ? Math.round(mine.reduce((sum, u) => sum + uploadPercent(u), 0) / mine.length)
    : 0;
  // Match saved files back to library items (for the Gallery's focus and
  // real dimensions); the result text alone still renders the pictures.
  const itemFor = (path: string) => library?.find((m) => m.path === path) ?? null;

  return (
    <div className={`toolcall media-tool ${item.running ? "running" : failed ? "failed" : "done"}`}>
      <div className="toolcall-head static">
        <span className="toolcall-icon">
          <Icon size={15} />
        </span>
        <span className="toolcall-summary">
          <span className="toolcall-verb">{verb}</span>
          {detail && (
            <span className="toolcall-target" title={detail}>
              {detail}
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
        </span>
      </div>
      <div className="media-body">
        {g.prompt && (
          <p className="media-prompt">
            {prompt}
            {long && (
              <button type="button" className="media-more" onClick={() => setExpanded((e) => !e)}>
                {expanded ? "less" : "more"}
              </button>
            )}
          </p>
        )}
        {g.refs.length > 0 && (
          <div className="media-refs">
            {g.refs.map((r) => (
              <span key={r} className="media-ref-chip" title={r}>
                {r}
              </span>
            ))}
          </div>
        )}
        {item.running && pending.length > 0 && (
          <div className="media-uploads" aria-label="Uploading references">
            <div className="media-uploads-head">
              Uploading references · {settled} of {mine.length} · {overallPercent}%
            </div>
            {mine.map((u) => (
              <UploadRow key={u.id} upload={u} compact />
            ))}
          </div>
        )}
        {item.running && pending.length === 0 && (
          <div className={`media-tiles count-${g.count}`}>
            {Array.from({ length: g.count }, (_, i) => (
              <div
                key={i}
                className="media-tile skeleton"
                style={{ aspectRatio: aspectFor(null, g.aspectRatio ?? (g.kind === "video" ? "16:9" : undefined)) }}
                aria-label="rendering"
              />
            ))}
          </div>
        )}
        {!item.running && files.length > 0 && workspace && (
          <div className={`media-tiles count-${files.length}`}>
            {files.map((f) => {
              const known = itemFor(f.path);
              return (
                <WorkspaceMedia
                  key={f.path}
                  workspace={workspace}
                  path={f.path}
                  poster={f.poster}
                  kind={f.kind}
                  aspect={aspectFor(known, g.aspectRatio ?? (f.kind === "video" ? "16:9" : undefined))}
                  onOpen={() => (known ? openGallery(known.id) : openInViewer([f.path]))}
                />
              );
            })}
          </div>
        )}
        {needsKey && (
          <div className="media-nokey">
            <KeyRound size={15} />
            <span>
              Media generation needs your Oxen API key.{" "}
              <button type="button" className="media-more" onClick={() => openSettings("connection")}>
                Add it in Settings → Connection
              </button>
            </span>
          </div>
        )}
        {failed && !needsKey && <pre className="toolcall-code media-error">{clamp(item.result)}</pre>}
        {!item.running && !failed && !needsKey && files.length === 0 && item.result && (
          <pre className="toolcall-code">{clamp(item.result)}</pre>
        )}
      </div>
    </div>
  );
}

/** One saved output: an image tile, or a video with its poster frame. Loads
 *  through the same asset route as the Files dock. */
function WorkspaceMedia({
  workspace,
  path,
  poster,
  kind,
  aspect,
  onOpen,
}: {
  workspace: string;
  path: string;
  poster?: string;
  kind: "image" | "video";
  aspect: string;
  onOpen: () => void;
}) {
  const src = useAssetSrc(workspace, path, 0);
  const posterSrc = useAssetSrc(workspace, poster ?? "", 0);
  const name = path.split("/").pop() ?? path;
  // Every rendered output is a drag source: drop it on the chat to attach
  // it as a reference for the next generation.
  const abs = `${workspace}/${path}`;
  const dragStart = (e: DragEvent) => setDragPaths(e.dataTransfer, [abs]);
  if (kind === "video") {
    return (
      <div
        className="media-tile video"
        style={{ aspectRatio: aspect }}
        title={`${name} — drag into the chat to attach`}
        draggable
        onDragStart={dragStart}
      >
        {src && (
          <video src={src} controls preload="metadata" poster={poster && posterSrc ? posterSrc : undefined} />
        )}
        <button type="button" className="media-tile-open" onClick={onOpen} aria-label={`Open ${name} in the gallery`}>
          gallery ›
        </button>
      </div>
    );
  }
  return (
    <button
      type="button"
      className="media-tile"
      style={{ aspectRatio: aspect }}
      onClick={onOpen}
      title={`${name} — open in the gallery · drag into the chat to attach`}
      draggable
      onDragStart={dragStart}
    >
      {src ? <img src={src} alt={name} loading="lazy" draggable={false} /> : <span className="media-tile-loading" aria-hidden />}
    </button>
  );
}

/** A plan update in the thread: a non-collapsing card showing the checklist
 *  snapshot for that call, with a "done/total" count in the header. */
function PlanToolCard({ item, a }: { item: ToolItem; a: Record<string, unknown> }) {
  // Rows only animate while this chat's run is live — a card in a finished
  // (or errored/cancelled) chat is a historical snapshot and must not spin.
  const running = useStore((s) =>
    s.session ? s.runStatus[s.session.session_id] === "running" : false,
  );
  const items = planItemsFromArgs(a);
  if (!items) return null;
  const { done, total } = planProgress(items);
  return (
    <div className={`toolcall plan-card ${item.running ? "running" : "done"}`}>
      <div className="toolcall-head static">
        <span className="toolcall-icon">
          <ListChecks size={15} />
        </span>
        <span className="toolcall-summary">
          <span className="toolcall-verb">{item.running ? "Updating plan" : "Plan"}</span>
        </span>
        <span className="toolcall-meta">
          <span className="plan-progress">
            {done}/{total}
          </span>
        </span>
      </div>
      <PlanChecklist items={items} live={running} />
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
        <Check size={15} />
        <span>Key saved — ask me to search again and it’ll work.</span>
      </div>
    );
  }

  return (
    <div className="ws-key">
      <div className="ws-key-head">
        <KeyRound size={15} />
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

function present(name: string, a: Record<string, unknown>, root?: string | null): Presentation {
  const s = (k: string) => (typeof a[k] === "string" ? (a[k] as string) : undefined);
  switch (name) {
    case "read_file":
      return { icon: FileText, verb: "Read", target: pathLabel(s("path"), root) };
    case "write_file":
      return { icon: FilePlus2, verb: "Wrote", target: pathLabel(s("path"), root) };
    case "edit_file":
      return { icon: PencilLine, verb: "Edited", target: pathLabel(s("path"), root) };
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
    case MEDIA_MODELS_TOOL:
      return { icon: Images, verb: "Browsed media models", target: s("id") ?? s("query") };
    case MEDIA_STATUS_TOOL:
      return { icon: ListVideo, verb: a.cancel === true ? "Cancelled a generation" : "Checked generations" };
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

function pathLabel(path?: string, root?: string | null): string | undefined {
  if (!path) return undefined;
  // Show the path relative to the project (e.g. `src/main.rs`), or the full
  // path when it lives outside the project. The full text is in the title.
  return relPath(path, root);
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
