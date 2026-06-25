import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowDown, FileText } from "lucide-react";
import { onFileDrop, pickAttachments } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { basename } from "../../lib/format";
import { ThreadItem } from "./ThreadItem";
import { Composer } from "./Composer";
import { Queue } from "./Queue";
import { Hero } from "./Hero";
import { QuestionPrompt } from "../questions/QuestionPrompt";
import { type Item } from "./thread";
import "./chat.css";

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

export function Chat() {
  const sessionId = useStore((s) => s.session?.session_id);
  // Read the current chat's thread / queue / run state straight from the store —
  // it owns them so they persist while this chat streams in the background.
  const items = useStore((s) => (s.session ? s.threads[s.session.session_id] : undefined)) ?? NO_ITEMS;
  const queue = useStore((s) => (s.session ? s.queues[s.session.session_id] : undefined)) ?? NO_QUEUE;
  const running = useStore((s) => !!s.session && s.runStatus[s.session.session_id] === "running");
  const send = useStore((s) => s.send);
  const setQueue = useStore((s) => s.setQueue);
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

  // Send now (with any staged attachments) or, if this chat is mid-turn, queue
  // it — leaving staged attachments in place for when the turn frees up.
  function submit(text: string) {
    stick.current = true;
    setAtBottom(true);
    if (running) {
      send(text);
      return;
    }
    const paths = attachments.map((a) => a.path);
    setAttachments([]);
    send(text, paths);
  }

  return (
    <main className="chat">
      <div className="chat-titlebar" data-tauri-drag-region />
      <div className="messages-wrap">
        {showReopenCanvas && lastCanvas && (
          <button
            className="reopen-canvas"
            onClick={() => setActiveCanvas(lastCanvas.id)}
            title={`Reopen canvas: ${lastCanvas.title}`}
          >
            <FileText size={14} />
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

      <Queue items={queue} onChange={setQueue} />
      {attachments.length > 0 && (
        <div className="attachments">
          {attachments.map((a, i) => (
            <span className="attachment-chip" key={`${a.path}-${i}`}>
              📎 {a.name}
              <button
                className="attachment-x"
                aria-label={`Remove ${a.name}`}
                onClick={() => setAttachments((prev) => prev.filter((_, j) => j !== i))}
              >
                ✕
              </button>
            </span>
          ))}
        </div>
      )}
      <QuestionPrompt />
      <Composer busy={running} onSend={submit} onAttach={attach} />
    </main>
  );
}
