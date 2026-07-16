import { useEffect, useLayoutEffect, useRef, useState, type DragEvent } from "react";
import { ArrowDown, FileCode2, FileText, SearchCode } from "lucide-react";
import { fsReadFile, onFileDrop, pickAttachments } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { basename } from "../../lib/format";
import { snippetLabel } from "../../lib/snippets";
import { getDragPaths, hasDragPaths } from "../files/dnd";
import type { CodeSnippet } from "../../lib/types";
import { ThreadItem } from "./ThreadItem";
import { FleetPanel } from "./FleetPanel";
import { Plan } from "./Plan";
import { Composer } from "./Composer";
import { Queue } from "./Queue";
import { Hero } from "./Hero";
import { GameDock } from "./GameDock";
import { TokenMeter } from "./TokenMeter";
import { StreamingWrite } from "./StreamingWrite";
import { AttachmentImage } from "./AttachmentImage";
import { isImagePath, isVideoPath } from "../../lib/attachments";
import { QuestionPrompt } from "../questions/QuestionPrompt";
import { ApprovalPrompt } from "../approvals/ApprovalPrompt";
import { type Item } from "./thread";
import "./chat.css";
import { dispatchSlashCommand } from "./slashDispatch";

const EXAMPLES = [
  "Explain this codebase",
  "Add a unit test for the parser",
  "Find and fix the failing test",
  "Summarize recent git changes",
];

// Stable empty defaults so narrow selectors don't return a fresh array each
// render (which would thrash zustand's equality check).
const NO_ITEMS: Item[] = [];
const NO_QUEUE: string[] = [];
const NO_SNIPPETS: CodeSnippet[] = [];

/** Cap on a whole file staged as context via drag-and-drop, so a dropped
 *  lockfile can't quietly eat the context window. */
const SNIPPET_FILE_CAP = 30_000;

export function Chat() {
  const sessionId = useStore((s) => s.session?.session_id);
  // Read the current chat's thread / queue / run state straight from the store —
  // it owns them so they persist while this chat streams in the background.
  const items = useStore((s) => (s.session ? s.threads[s.session.session_id] : undefined)) ?? NO_ITEMS;
  const queueEntries = useStore((s) => (s.session ? s.queues[s.session.session_id] : undefined));
  const queue = queueEntries?.map((q) => q.text) ?? NO_QUEUE;
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  // A running code review's live progress (which step, what the agent is doing).
  const review = useStore((s) => (s.session ? s.codeReview[s.session.session_id] : undefined));
  const send = useStore((s) => s.send);
  const stop = useStore((s) => s.stop);
  const setQueue = useStore((s) => s.setQueue);
  // Code selections staged from the editor (or files dropped from the tree),
  // shown as chips and baked into the next prompt as context.
  const snippets =
    useStore((s) => (s.session ? s.snippets[s.session.session_id] : undefined)) ?? NO_SNIPPETS;
  const addSnippet = useStore((s) => s.addSnippet);
  const removeSnippet = useStore((s) => s.removeSnippet);
  const workspace = useStore((s) => s.session?.workspace);
  const addNotice = useStore((s) => s.addNotice);
  // The floating game dock lets you play a round while a turn streams, so a long
  // run doesn't send you off to another app. Its toggle lives in the title bar.
  const gameDockOpen = useStore((s) => s.gameDockOpen);
  // The most recent canvas in this chat, and whether the panel is currently
  // showing it — used to offer a one-click "reopen canvas" when it's closed.
  const sessionCanvases = useStore((s) => (s.session ? s.canvases[s.session.session_id] : undefined));
  const activeCanvasId = useStore((s) => (s.session ? s.activeCanvas[s.session.session_id] : undefined));
  const canvasWriting = useStore((s) => !!s.session && !!s.canvasWriting[s.session.session_id]);
  const setActiveCanvas = useStore((s) => s.setActiveCanvas);
  const lastCanvas = sessionCanvases?.length ? sessionCanvases[sessionCanvases.length - 1] : null;
  const canvasShowing =
    canvasWriting || (!!activeCanvasId && !!sessionCanvases?.some((d) => d.id === activeCanvasId));
  const showReopenCanvas = !!lastCanvas && !canvasShowing;

  const [attachments, setAttachments] = useState<{ path: string; name: string }[]>([]);
  const [now, setNow] = useState(() => Date.now()); // drives running tool timers
  const [atBottom, setAtBottom] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Mirrors `atBottom` so the auto-scroll effect can read the latest value
  // without listing it as a dependency (which would re-snap on every toggle).
  const stick = useRef(true);

  function isNearBottom(el: HTMLElement) {
    return el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }

  // Track whether the user is parked at the bottom. Scrolling up unsticks the
  // view so streaming output stops yanking them back down.
  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    stick.current = isNearBottom(el);
    setAtBottom(stick.current);
  }

  function scrollToBottom(smooth = true) {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" });
    stick.current = true;
    setAtBottom(true);
  }

  // Switching chats starts pinned to the bottom of that chat's thread.
  useEffect(() => {
    stick.current = true;
    setAtBottom(true);
  }, [sessionId]);

  // Follow new content only while the user is parked at the bottom.
  useLayoutEffect(() => {
    if (!stick.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [items]);

  // Advance running tool timers once a second while this chat is in flight.
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [running]);

  // Add files (from an OS drop or the picker) to the pending attachments,
  // skipping any path already staged so re-picking is idempotent.
  function addAttachments(paths: string[]) {
    setAttachments((prev) => {
      const have = new Set(prev.map((a) => a.path));
      const next = paths.filter((p) => !have.has(p)).map((path) => ({ path, name: basename(path) }));
      return next.length ? [...prev, ...next] : prev;
    });
  }

  // Subscribe to OS file drops for the active composer.
  useEffect(() => {
    const unDrop = onFileDrop(addAttachments);
    return () => {
      unDrop.then((fn) => fn());
    };
  }, []);

  async function attach() {
    try {
      addAttachments(await pickAttachments());
    } catch {
      /* user cancelled or the dialog failed — nothing to stage */
    }
  }

  // A text file dragged in from the Files tree becomes a staged snippet (its
  // whole content, capped) rather than a binary attachment.
  async function stageFileSnippet(absPath: string) {
    if (!workspace || !absPath.startsWith(`${workspace}/`)) return;
    const rel = absPath.slice(workspace.length + 1);
    try {
      const body = await fsReadFile(workspace, rel);
      const code = body.content.slice(0, SNIPPET_FILE_CAP);
      addSnippet({ path: rel, start: 1, end: code.split("\n").length, code });
    } catch {
      /* binary or unreadable — nothing to stage */
    }
  }

  // Workspace files dragged from the Files tree / gallery tiles (in-app HTML5
  // drag; OS drops arrive separately through onFileDrop above).
  function onInternalDrop(e: DragEvent) {
    const paths = getDragPaths(e.dataTransfer);
    if (!paths.length) return;
    e.preventDefault();
    const images = paths.filter(isImagePath);
    const videos = paths.filter(isVideoPath);
    const texts = paths.filter((p) => !isImagePath(p) && !isVideoPath(p));
    if (images.length) addAttachments(images);
    for (const p of texts) void stageFileSnippet(p);
    if (videos.length) addNotice("Videos can't be sent to the model yet — view them in the Editor pane.");
  }

  // Send now (with any staged attachments) or, if this chat is mid-turn, queue
  // the prompt and the same attachment set so the eventual turn is identical.
  async function submit(text: string) {
    stick.current = true;
    setAtBottom(true);
    if (await dispatchSlashCommand(text)) return;
    const paths = attachments.map((a) => a.path);
    setAttachments([]);
    send(text, paths);
  }

  return (
    <main
      className="chat"
      onDragOver={(e) => {
        if (hasDragPaths(e.dataTransfer)) e.preventDefault();
      }}
      onDrop={onInternalDrop}
    >
      <div className="messages-wrap">
        <Plan />
        {showReopenCanvas && lastCanvas && (
          <button
            className="reopen-canvas"
            onClick={() => setActiveCanvas(lastCanvas.id)}
            title={`Reopen canvas: ${lastCanvas.title}`}
          >
            <FileText size={15} />
            <span>{lastCanvas.title}</span>
          </button>
        )}
        <div className="messages" ref={scrollRef} onScroll={onScroll}>
          {items.length === 0 ? (
            <div className="chat-empty">
              <Hero examples={EXAMPLES} busy={running} onPick={submit} />
            </div>
          ) : (
            <div className="thread">
              {items.map((it) => (
                <ThreadItem key={it.id} item={it} now={now} />
              ))}
              <StreamingWrite />
            </div>
          )}
        </div>
        {items.length > 0 && !atBottom && (
          <button
            className="scroll-bottom"
            onClick={() => scrollToBottom()}
            aria-label="Scroll to latest"
            title="Scroll to latest"
          >
            <ArrowDown size={18} />
          </button>
        )}
      </div>

      {review && (
        <div className="review-progress" role="status">
          <SearchCode size={14} className="review-progress-icon" />
          <span className="review-progress-step">
            Code review
            {review.total > 0
              ? ` — step ${review.index + 1}/${review.total}: ${review.step}`
              : ` — ${review.step}`}
          </span>
          {review.activity && <span className="review-progress-activity">{review.activity}</span>}
        </div>
      )}
      <FleetPanel />
      <Queue items={queue} onChange={setQueue} />
      {snippets.length > 0 && (
        <div className="attachments">
          {snippets.map((sn, i) => (
            <span
              className="attachment-chip snippet-chip"
              key={`${sn.path}-${sn.start}-${i}`}
              title={sn.code.length > 400 ? `${sn.code.slice(0, 400)}…` : sn.code}
            >
              <FileCode2 size={12} aria-hidden="true" /> {snippetLabel(sn)}
              <button
                className="attachment-x"
                aria-label={`Remove snippet ${snippetLabel(sn)}`}
                onClick={() => removeSnippet(i)}
              >
                ✕
              </button>
            </span>
          ))}
        </div>
      )}
      {attachments.length > 0 && (
        <div className="attachments">
          {attachments.map((a, i) => {
            const remove = () => setAttachments((prev) => prev.filter((_, j) => j !== i));
            return isImagePath(a.path) ? (
              <span className="attachment-thumb" key={`${a.path}-${i}`} title={a.name}>
                <AttachmentImage src={a.path} alt={a.name} className="attachment-thumb-img" />
                <span className="attachment-thumb-name">{a.name}</span>
                <button className="attachment-x" aria-label={`Remove ${a.name}`} onClick={remove}>
                  ✕
                </button>
              </span>
            ) : (
              <span className="attachment-chip" key={`${a.path}-${i}`}>
                📎 {a.name}
                <button className="attachment-x" aria-label={`Remove ${a.name}`} onClick={remove}>
                  ✕
                </button>
              </span>
            );
          })}
        </div>
      )}
      <QuestionPrompt />
      <ApprovalPrompt />
      {gameDockOpen && items.length > 0 && <GameDock />}
      {items.length > 0 && <TokenMeter />}
      <Composer
        busy={running}
        focusKey={items.length === 0 ? sessionId : undefined}
        onSend={submit}
        onStop={stop}
        onAttach={attach}
      />
    </main>
  );
}
