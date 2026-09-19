import { useEffect, useRef, useState, type DragEvent } from "react";
import { ArrowDown, FileCode2, FileText, SearchCode } from "lucide-react";
import { fsReadFile, pickAttachments, stageDroppedFile } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { basename } from "../../lib/format";
import { snippetLabel } from "../../lib/snippets";
import { getDragPaths, hasDragPaths } from "../files/dnd";
import type { CodeSnippet } from "../../lib/types";
import { ThreadItem } from "./ThreadItem";
import { FleetPanel } from "./FleetPanel";
import { AgentView } from "./AgentView";
import { TasksPanel } from "./TasksPanel";
import { MediaPanel } from "./MediaPanel";
import { Plan } from "./Plan";
import { ThreadStatus } from "./ThreadStatus";
import { ChatTabs } from "../tabs/ChatTabs";
import { Composer } from "./Composer";
import { Queue } from "./Queue";
import { Hero } from "./Hero";
import { GameDock } from "./GameDock";
import { TokenMeter } from "./TokenMeter";
import { StreamingWrite } from "./StreamingWrite";
import { isMediaPath } from "../../lib/attachments";
import { QuestionPrompt } from "../questions/QuestionPrompt";
import { ApprovalPrompt } from "../approvals/ApprovalPrompt";
import { type Item } from "./thread";
import "./chat.css";
import { useChatScroll } from "./useChatScroll";
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

/** A File's bytes; FileReader is the fallback where `arrayBuffer` is missing. */
function readBytes(file: File): Promise<ArrayBuffer> {
  if (typeof file.arrayBuffer === "function") return file.arrayBuffer();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as ArrayBuffer);
    reader.onerror = () => reject(reader.error ?? new Error("could not read the file"));
    reader.readAsArrayBuffer(file);
  });
}

export function Chat() {
  const sessionId = useStore((s) => s.session?.session_id);
  // Read the current chat's thread / queue / run state straight from the store —
  // it owns them so they persist while this chat streams in the background.
  const items = useStore((s) => (s.session ? s.threads[s.session.session_id] : undefined)) ?? NO_ITEMS;
  // A local command (e.g. `/location`) can drop a notice into a chat nobody
  // has written in yet; only a real message swaps the hero for the thread.
  const started = items.some((it) => it.kind !== "notice");
  const queueEntries = useStore((s) => (s.session ? s.queues[s.session.session_id] : undefined));
  const queue = queueEntries?.map((q) => q.text) ?? NO_QUEUE;
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  // A running code review's live progress (which step, what the agent is doing).
  const review = useStore((s) => (s.session ? s.codeReview[s.session.session_id] : undefined));
  const send = useStore((s) => s.send);
  const addNotice = useStore((s) => s.addNotice);
  const stop = useStore((s) => s.stop);
  const setQueue = useStore((s) => s.setQueue);
  // Code selections staged from the editor (or files dropped from the tree),
  // shown as chips and baked into the next prompt as context.
  const snippets =
    useStore((s) => (s.session ? s.snippets[s.session.session_id] : undefined)) ?? NO_SNIPPETS;
  const addSnippet = useStore((s) => s.addSnippet);
  const removeSnippet = useStore((s) => s.removeSnippet);
  const workspace = useStore((s) => s.session?.workspace);
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

  // A subagent opened from the lane strip takes the thread column over: its
  // transcript and composer, under the tabs, until Esc or the back chip.
  const agentLane = useStore((s) => (s.session ? s.agentView[s.session.session_id] : undefined));
  const [attachments, setAttachments] = useState<{ path: string; name: string }[]>([]);
  const { scrollRef, contentRef, paused, scrollToBottom, onScroll, onWheel } =
    useChatScroll(sessionId, items);

  // Add files (from an OS drop or the picker) to the pending attachments,
  // skipping any path already staged so re-picking is idempotent.
  function addAttachments(paths: string[]) {
    setAttachments((prev) => {
      const have = new Set(prev.map((a) => a.path));
      const next = paths.filter((p) => !have.has(p)).map((path) => ({ path, name: basename(path) }));
      return next.length ? [...prev, ...next] : prev;
    });
  }

  // A file dropped from the OS (Finder, another app) arrives as File objects
  // with no path — the webview's native drop hook is off so in-app drags
  // work — so its bytes are staged into the project and attached by path.
  async function stageOsFiles(files: FileList) {
    for (const file of Array.from(files)) {
      try {
        const bytes = new Uint8Array(await readBytes(file));
        const path = await stageDroppedFile(file.name, bytes);
        addAttachments([path]);
      } catch (e) {
        addNotice(`Couldn't attach ${file.name}: ${String(e)}`);
      }
    }
  }

  // Files another surface staged for the next message (the Gallery's "Use as
  // reference"): take them as soon as they appear.
  const pendingAttachments = useStore((s) => s.pendingAttachments);
  const takePendingAttachments = useStore((s) => s.takePendingAttachments);
  useEffect(() => {
    if (pendingAttachments.length) addAttachments(takePendingAttachments());
  }, [pendingAttachments, takePendingAttachments]);

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
  // Highlight the composer while a file drag hovers the chat (in-app paths
  // or OS files). A counter, not a flag: dragenter/dragleave fire for every
  // child crossed, and only the outermost leave should clear it.
  const [dropActive, setDropActive] = useState(false);
  const dragDepth = useRef(0);
  const isFileDrag = (dt: DataTransfer) => hasDragPaths(dt) || dt.types.includes("Files");
  function onDragEnter(e: DragEvent) {
    if (!isFileDrag(e.dataTransfer)) return;
    dragDepth.current += 1;
    setDropActive(true);
  }
  function onDragLeave(e: DragEvent) {
    if (!isFileDrag(e.dataTransfer)) return;
    dragDepth.current = Math.max(0, dragDepth.current - 1);
    if (dragDepth.current === 0) setDropActive(false);
  }
  function onDragOver(e: DragEvent) {
    if (isFileDrag(e.dataTransfer)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    }
  }

  function onInternalDrop(e: DragEvent) {
    dragDepth.current = 0;
    setDropActive(false);
    const paths = getDragPaths(e.dataTransfer);
    if (!paths.length) {
      // Not an in-app drag: OS files, if any.
      if (e.dataTransfer.files && e.dataTransfer.files.length) {
        e.preventDefault();
        void stageOsFiles(e.dataTransfer.files);
      }
      return;
    }
    e.preventDefault();
    // Images, video, and audio all become attachments: the model sees the
    // images, and every one of them can be a reference for generation.
    const media = paths.filter(isMediaPath);
    const texts = paths.filter((p) => !isMediaPath(p));
    if (media.length) addAttachments(media);
    for (const p of texts) void stageFileSnippet(p);
  }

  // Send now (with any staged attachments) or, if this chat is mid-turn, queue
  // the prompt and the same attachment set so the eventual turn is identical.
  async function submit(text: string) {
    scrollToBottom();
    if (await dispatchSlashCommand(text)) return;
    const paths = attachments.map((a) => a.path);
    setAttachments([]);
    send(text, paths);
  }

  if (sessionId && agentLane) {
    return (
      <main className="chat">
        <ChatTabs />
        <AgentView key={agentLane} session={sessionId} lane={agentLane} />
      </main>
    );
  }

  return (
    <main
      className={`chat${dropActive ? " chat-drop-active" : ""}`}
      onDragEnter={onDragEnter}
      onDragLeave={onDragLeave}
      onDragOver={onDragOver}
      onDrop={onInternalDrop}
    >
      <ChatTabs />
      <div className="messages-wrap">
        <ThreadStatus />
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
        <div className="messages" ref={scrollRef} onScroll={onScroll} onWheel={onWheel}>
          {!started ? (
            <div className="chat-empty" ref={contentRef}>
              <Hero examples={EXAMPLES} busy={running} onPick={submit} />
              {items.map((it) => (
                <ThreadItem key={it.id} item={it} />
              ))}
            </div>
          ) : (
            <div className="thread" ref={contentRef}>
              {items.map((it) => (
                <ThreadItem key={it.id} item={it} />
              ))}
              <StreamingWrite />
            </div>
          )}
        </div>
        {started && paused && (
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
      <TasksPanel />
      <MediaPanel />
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
      <QuestionPrompt />
      <ApprovalPrompt />
      {gameDockOpen && started && <GameDock />}
      {started && <TokenMeter />}
      <Composer
        busy={running}
        focusKey={started ? undefined : sessionId}
        onSend={submit}
        onStop={stop}
        onAttach={attach}
        onDragOver={onDragOver}
        onDrop={onInternalDrop}
        attachments={attachments}
        onRemoveAttachment={(i) => setAttachments((prev) => prev.filter((_, j) => j !== i))}
        onClearAttachments={() => setAttachments([])}
      />
    </main>
  );
}
