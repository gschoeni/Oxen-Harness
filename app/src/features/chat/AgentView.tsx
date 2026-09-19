// A subagent, opened in place of the chat's thread column.
//
// A lane is a session of its own, so it gets the chat's own reading surface:
// the same thread items and the same composer, pointed at the lane. The bar
// above says where you are (the chat › the agent), what the agent is doing,
// and offers the few actions a lane has — step between siblings, stop it,
// review its changes, or open the raw transcript. Esc (or the back chip)
// returns to the chat, which keeps streaming underneath the whole time.
//
// The composer changes meaning with the agent's state: while it runs, Enter
// sends a direction the agent picks up on its next step; once it has finished,
// Enter follows it up and the lane runs again. Either way the message shows in
// the thread at once, and the persisted transcript takes over as it lands.

import { useCallback, useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { ArrowDown, ChevronDown, ChevronLeft, ChevronUp, FileDiff, ScrollText, Square, X } from "lucide-react";
import { agentPatch, applyAgentPatch, followUpAgent, sessionMessages } from "../../lib/ipc";
import { fleetsFor, useStore } from "../../lib/store";
import { compactTokens } from "../../lib/format";
import type { ChatMessage } from "../../lib/types";
import { agentRows, isActive, statusLabel } from "./agentRows";
import { transcriptToItems, uid, type Item } from "./thread";
import { ThreadItem } from "./ThreadItem";
import { Composer } from "./Composer";
import { ThinkingIndicator } from "./ThinkingIndicator";
import { useChatScroll } from "./useChatScroll";
import "./agentview.css";

/** How often a running lane's transcript is re-read (it persists as it runs). */
const FOLLOW_MS = 1500;

/** A message typed here that the persisted transcript has not caught up with
 *  yet. `after` is how many user messages the transcript held when it was
 *  sent, so the bubble retires once the transcript shows it in place. */
interface Pending {
  id: string;
  text: string;
  kind: "direction" | "follow-up";
  after: number;
}

/** A send that did not land, kept with its text so one click retries it the
 *  right way (an agent that finished mid-send takes a follow-up instead). */
interface Failed {
  text: string;
  message: string;
  followUp: boolean;
}

const clock = (secs: number) =>
  secs >= 60 ? `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, "0")}s` : `${secs}s`;

export function AgentView({ session, lane }: { session: string; lane: string }) {
  const fleets = useStore((s) => s.fleets);
  const agents = useStore((s) => s.agents[session]);
  const refresh = useStore((s) => s.refreshAgents);
  const close = useStore((s) => s.closeAgent);
  const open = useStore((s) => s.openAgent);
  const stopLane = useStore((s) => s.stopLane);
  const steer = useStore((s) => s.steerLane);
  const openInspector = useStore((s) => s.openInspector);

  const rows = useMemo(() => agentRows(agents ?? [], fleetsFor(fleets, session)), [agents, fleets, session]);
  const row = rows.find((r) => r.id === lane);
  const running = !!row && isActive(row.status);
  // Siblings you can step to: every lane that has started (queued ones have no id yet).
  const openable = rows.filter((r) => r.id);
  const at = openable.findIndex((r) => r.id === lane);
  const previous = at > 0 ? openable[at - 1] : undefined;
  const next = at >= 0 && at + 1 < openable.length ? openable[at + 1] : undefined;

  useEffect(() => {
    void refresh(session);
  }, [session, lane, refresh]);

  // --- the transcript ------------------------------------------------------
  const [messages, setMessages] = useState<ChatMessage[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const load = useCallback(async () => {
    try {
      setMessages(await sessionMessages(lane));
      setLoadError("");
    } catch (e) {
      setLoadError(String(e));
    }
  }, [lane]);
  useEffect(() => {
    setMessages(null);
    void load();
  }, [load]);
  // A follow-up in flight: the lane runs again, and the view follows it even
  // before the fleet events say so.
  const [following, setFollowing] = useState(false);
  const live = running || following;
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => void load(), FOLLOW_MS);
    return () => {
      window.clearInterval(timer);
      void load();
    };
  }, [live, load]);
  // The lane settled (or was resumed): read once more so the reply is whole.
  const status = row?.status;
  useEffect(() => {
    void load();
  }, [status, load]);

  const items = useMemo(() => transcriptToItems(messages ?? []), [messages]);
  // The first user message is the brief the chat gave this agent — not
  // something you typed — so it reads as an assignment, not a bubble.
  const [assignment, rest] =
    items[0]?.kind === "user" ? [items[0], items.slice(1)] : [undefined, items];
  const userCount = items.filter((i) => i.kind === "user").length;

  const [pending, setPending] = useState<Pending[]>([]);
  useEffect(() => {
    if (!pending.length) return;
    const users = items.filter((i): i is Extract<Item, { kind: "user" }> => i.kind === "user");
    setPending((p) => p.filter((m) => !users.slice(m.after).some((u) => u.text === m.text)));
  }, [items, pending.length]);

  // Anything that adds height at the tail keeps the view pinned to the bottom
  // while the reader is following: new items, a pending bubble, the live footer.
  const tail = useMemo(() => [items, pending, live] as const, [items, pending, live]);
  const { scrollRef, contentRef, paused, scrollToBottom, onScroll, onWheel } = useChatScroll(lane, tail);

  // --- sending -----------------------------------------------------------
  const [failed, setFailed] = useState<Failed | null>(null);

  async function followUp(text: string) {
    const id = uid();
    setFailed(null);
    setPending((p) => [...p, { id, text, kind: "follow-up", after: userCount }]);
    setFollowing(true);
    try {
      await followUpAgent(session, lane, text);
    } catch (e) {
      setPending((p) => p.filter((m) => m.id !== id));
      setFailed({ text, message: String(e).replace(/^Error: /, ""), followUp: true });
    } finally {
      setFollowing(false);
      await refresh(session);
      await load();
    }
  }

  async function submit(text: string) {
    scrollToBottom();
    setFailed(null);
    if (!running) return followUp(text);
    try {
      if (await steer(session, lane, text)) {
        setPending((p) => [...p, { id: uid(), text, kind: "direction", after: userCount }]);
      } else {
        await refresh(session);
        setFailed({
          text,
          message: `${row?.label ?? "The agent"} finished before your message arrived.`,
          followUp: true,
        });
      }
    } catch (e) {
      setFailed({ text, message: String(e).replace(/^Error: /, ""), followUp: false });
    }
  }

  // --- stopping ------------------------------------------------------------
  const [stopping, setStopping] = useState(false);
  useEffect(() => {
    if (!running) setStopping(false);
  }, [running]);
  async function stop() {
    setStopping(true);
    try {
      if (!(await stopLane(session, lane))) await refresh(session);
    } catch (e) {
      setStopping(false);
      setFailed({ text: "", message: String(e), followUp: false });
    }
  }

  // --- the patch -----------------------------------------------------------
  const [patch, setPatch] = useState<string | null>(null);
  const [patchBusy, setPatchBusy] = useState(false);
  const [patchError, setPatchError] = useState("");
  const [applied, setApplied] = useState(false);
  useEffect(() => {
    setPatch(null);
    setApplied(false);
    setPatchError("");
  }, [lane, status]);
  async function togglePatch() {
    if (patch !== null) return setPatch(null);
    setPatchBusy(true);
    setPatchError("");
    try {
      setPatch(await agentPatch(session, lane));
    } catch (e) {
      setPatchError(String(e));
    } finally {
      setPatchBusy(false);
    }
  }
  async function apply() {
    if (patch === null) return;
    setPatchBusy(true);
    setPatchError("");
    try {
      await applyAgentPatch(session, lane, patch);
      setApplied(true);
    } catch (e) {
      setPatchError(String(e).replace(/^Error: /, ""));
    } finally {
      setPatchBusy(false);
    }
  }

  // Esc steps out: first out of the patch, then back to the chat. A composer
  // that used it (to clear a slash menu) marks the event handled.
  function onKeyDown(e: KeyboardEvent<HTMLElement>) {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    e.preventDefault();
    if (patch !== null) setPatch(null);
    else close(session);
  }

  const label = row?.label ?? "Agent";
  return (
    <section className="agent-view" aria-label={`Agent ${label}`} onKeyDown={onKeyDown}>
      <header className="agent-view-bar">
        <button
          className="icon-btn sm agent-view-back"
          onClick={() => close(session)}
          aria-label="Back to the chat"
          title="Back to the chat (Esc)"
        >
          <ChevronLeft size={16} />
        </button>
        <h2 className="agent-view-name" title={lane}>
          {label}
        </h2>
        {row && (
          <span className={`agent-view-status ${row.status}`}>
            {running && <span className="agent-hub-dot" aria-hidden="true" />}
            {stopping ? "Stopping…" : statusLabel(row.status)}
          </span>
        )}
        {row && (
          <span className="agent-view-meta">
            {row.model && <span>{row.model}</span>}
            {row.tokens > 0 && <span>{compactTokens(row.tokens)} tokens</span>}
            {row.elapsed_secs > 0 && <span>{clock(row.elapsed_secs)}</span>}
          </span>
        )}
        <span className="agent-view-spacer" />
        <div className="agent-view-actions">
          {openable.length > 1 && (
            <nav className="agent-view-switch" aria-label="Other agents">
              <button
                onClick={() => previous && open(session, previous.id)}
                disabled={!previous}
                aria-label="Previous agent"
                title={previous ? previous.label : undefined}
              >
                <ChevronUp size={15} />
              </button>
              <span>
                {at + 1} / {openable.length}
              </span>
              <button
                onClick={() => next && open(session, next.id)}
                disabled={!next}
                aria-label="Next agent"
                title={next ? next.label : undefined}
              >
                <ChevronDown size={15} />
              </button>
            </nav>
          )}
          {row?.has_patch && !running && (
            <button
              className={patch !== null ? "active" : ""}
              onClick={togglePatch}
              disabled={patchBusy}
              aria-pressed={patch !== null}
            >
              <FileDiff size={14} /> Changes
            </button>
          )}
          <button onClick={() => openInspector(lane)} aria-label="Raw transcript" title="Raw transcript">
            <ScrollText size={14} />
          </button>
          {running && (
            <button className="agent-view-stop" onClick={stop} disabled={stopping} aria-label={`Stop ${label}`}>
              <Square size={11} fill="currentColor" /> Stop
            </button>
          )}
        </div>
      </header>

      <div className="messages-wrap">
        <div className="messages" ref={scrollRef} onScroll={onScroll} onWheel={onWheel}>
          <div className="thread agent-view-thread" ref={contentRef}>
            {assignment && (
              <div className="agent-view-assignment">
                <span className="agent-view-assignment-label">Assignment</span>
                {assignment.text}
              </div>
            )}
            {rest.map((it) => (
              <ThreadItem key={it.id} item={it} />
            ))}
            {pending.map((m) => (
              <div className="msg user pending" key={m.id}>
                <div className="msg-user-text">{m.text}</div>
                <span className="agent-view-pending-note" role="status">
                  {m.kind === "direction" ? "Queued for its next step" : "Sending…"}
                </span>
              </div>
            ))}
            {live && (
              <div className="agent-view-live">
                <ThinkingIndicator writing={!!row?.activity} />
                {row?.activity && <span className="agent-view-activity">{row.activity}</span>}
              </div>
            )}
            {!live && row?.stop && <div className="msg notice">{row.stop}</div>}
            {messages === null && !loadError && <div className="msg notice">Loading the transcript…</div>}
            {loadError && (
              <div className="msg notice" role="alert">
                {loadError}
              </div>
            )}
            {agents && !row && messages !== null && (
              <div className="msg notice">This agent is no longer part of the chat.</div>
            )}
          </div>
        </div>
        {paused && (
          <button className="scroll-bottom" onClick={() => scrollToBottom()} aria-label="Scroll to latest">
            <ArrowDown size={18} />
          </button>
        )}
      </div>

      {patch !== null && (
        <div className="agent-view-patch">
          <div className="agent-view-patch-card">
            <div className="agent-view-patch-head">
              <FileDiff size={13} />
              <strong>Proposed changes</strong>
              <span className="agent-view-spacer" />
              <button
                className="agent-view-apply"
                disabled={applied || patchBusy || !patch}
                onClick={apply}
              >
                {applied ? "Applied" : patchBusy ? "Applying…" : "Apply changes"}
              </button>
              <button onClick={() => setPatch(null)} aria-label="Close changes">
                <X size={13} />
              </button>
            </div>
            <PatchBody patch={patch} />
          </div>
        </div>
      )}
      {(patchError || failed) && (
        <div className="agent-view-error" role="alert">
          <span>{patchError || failed?.message}</span>
          {failed?.followUp && failed.text && (
            <button onClick={() => followUp(failed.text)}>Send as a follow-up</button>
          )}
        </div>
      )}

      <Composer
        busy={live}
        focusKey={lane}
        onSend={(text) => void submit(text)}
        onStop={() => void stop()}
        toolbar={false}
        placeholder={{
          idle: `Follow up with ${label}…`,
          busy: `Give ${label} a direction… (it picks it up on its next step)`,
        }}
      />
    </section>
  );
}

/** A unified diff, colored by line: files, hunks, additions, removals. */
function PatchBody({ patch }: { patch: string }) {
  if (!patch) return <pre aria-label="Agent changes">No changes to apply.</pre>;
  return (
    <pre aria-label="Agent changes">
      {patch.split("\n").map((line, i) => {
        const kind = line.startsWith("+++") || line.startsWith("---") || line.startsWith("diff ")
          ? "file"
          : line.startsWith("@@")
            ? "hunk"
            : line.startsWith("+")
              ? "add"
              : line.startsWith("-")
                ? "del"
                : "";
        return (
          <span className={`diff-line ${kind}`} key={i}>
            {line}
            {"\n"}
          </span>
        );
      })}
    </pre>
  );
}
