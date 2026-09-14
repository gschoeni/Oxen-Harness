import { useEffect, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  CircleDashed,
  FileDiff,
  MessageSquare,
  Square,
  Users,
  X,
} from "lucide-react";
import { compactTokens } from "../../lib/format";
import { agentPatch, applyAgentPatch, followUpAgent } from "../../lib/ipc";
import { fleetsFor, useStore, type FleetView } from "../../lib/store";
import type { AgentSummary } from "../../lib/types";
import "./agents.css";

type Row = AgentSummary & {
  key: string;
  tail: string;
  activity: string;
  fleetIndex?: number;
};
const active = (status: string) => status === "running" || status === "queued";
const statusLabel = (status: string) =>
  ({
    running: "Working",
    queued: "Queued",
    done: "Done",
    partial: "Partial",
    cancelled: "Stopped",
    failed: "Failed",
    unknown: "Interrupted",
  })[status] ?? status;

/** Persisted rows and event snapshots share lane IDs, including across follow-ups. */
export function agentRows(
  agents: AgentSummary[],
  fleets: Array<[string, FleetView]>,
): Row[] {
  const rows = new Map<string, Row>();
  for (const agent of agents)
    rows.set(agent.id, {
      ...agent,
      key: agent.id,
      tail: "",
      activity: agent.summary,
    });
  for (const [fleetId, fleet] of fleets) {
    fleet.lanes.forEach((lane, index) => {
      const candidates = lane.id
        ? []
        : agents.filter((a) => a.fleet === fleetId && a.label === lane.name);
      const saved = lane.id
        ? rows.get(lane.id)
        : candidates.length === 1 &&
            fleet.lanes.filter((l) => l.name === lane.name).length === 1
          ? rows.get(candidates[0].id)
          : undefined;
      const key = lane.id || saved?.id || `${fleetId}:${index}`;
      if (
        saved &&
        (fleet.finished || (saved.fleet === fleetId && !active(saved.status)))
      )
        return;
      rows.set(key, {
        ...saved,
        key,
        id: lane.id || saved?.id || "",
        label: lane.name,
        fleet: fleetId,
        status: fleet.finished && active(lane.status) ? "unknown" : lane.status,
        summary: lane.activity,
        activity: lane.activity,
        tail: lane.tail,
        tokens: lane.tokens,
        rounds: saved?.rounds ?? 0,
        elapsed_secs: saved?.elapsed_secs ?? 0,
        created_at: saved?.created_at ?? 0,
        fleetIndex: index,
      });
    });
  }
  const all = [...rows.values()];
  const sorted: Row[] = [];
  const visited = new Set<string>();
  const visit = (row: Row) => {
    if (visited.has(row.key)) return;
    visited.add(row.key);
    sorted.push(row);
    all.filter((r) => r.parent === row.id && r.id !== row.id).forEach(visit);
  };
  all.filter((row) => !row.parent || !rows.has(row.parent)).forEach(visit);
  all.forEach(visit);
  return sorted;
}

export function FleetPanel() {
  const session = useStore((s) => s.session?.session_id);
  return session ? <AgentHub key={session} session={session} /> : null;
}

function AgentHub({ session }: { session: string }) {
  const fleets = useStore((s) => s.fleets);
  const agents = useStore((s) => s.agents[session]);
  const refresh = useStore((s) => s.refreshAgents);
  const [open, setOpen] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const mine = fleetsFor(fleets, session);
  const rows = agentRows(agents ?? [], mine);
  const knownIds = rows
    .filter((r) => active(r.status))
    .map((r) => r.id)
    .join(",");
  useEffect(() => {
    void refresh(session);
  }, [session, refresh, knownIds]);
  const working = rows.filter((r) => active(r.status)).length;
  const focused = rows.find((r) => r.key === selected);
  const budget = mine.filter(([, f]) => !f.finished).slice(-1)[0]?.[1].budget;
  if (!rows.length) return null;
  return (
    <section className="agent-hub" aria-label="Agents">
      <div className="agent-hub-header">
        <button
          className="agent-hub-heading"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
        >
          <Users size={14} />
          <strong>Agents</strong>
          <span>
            {working ? `${working} working` : `${rows.length} finished`}
          </span>
          {working > 0 && rows.length > working && (
            <span>· {rows.length - working} finished</span>
          )}
          <span className="agent-hub-spacer" />
          {budget && (
            <span
              className="agent-hub-budget"
              title={`${compactTokens(budget.tokens)} of ${compactTokens(budget.max_tokens)} shared tokens`}
            >
              {compactTokens(budget.tokens)} tokens
            </span>
          )}
          {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
        {working > 0 && (
          <StopAll
            key={mine
              .filter(([, f]) => !f.finished)
              .map(([id]) => id)
              .join(",")}
            session={session}
            fleets={mine.filter(([, f]) => !f.finished)}
          />
        )}
      </div>
      {open && (
        <>
          <div className="agent-hub-rows">
            {rows.map((row) => (
              <div
                key={row.key}
                className={`agent-hub-row ${row.key === selected ? "selected" : ""}`}
              >
                <button
                  className="agent-hub-select"
                  style={{ paddingLeft: 14 + Math.min(row.depth ?? 0, 4) * 16 }}
                  title={`Watch ${row.label}`}
                  onClick={() =>
                    setSelected(selected === row.key ? null : row.key)
                  }
                  aria-expanded={selected === row.key}
                >
                  <Glyph status={row.status} />
                  <span className="agent-hub-name">{row.label}</span>
                  <span className="agent-hub-activity">
                    {row.activity || statusLabel(row.status)}
                  </span>
                  <span className={`agent-hub-status ${row.status}`}>
                    {statusLabel(row.status)}
                  </span>
                </button>
                {active(row.status) && row.id && (
                  <StopButton session={session} row={row} />
                )}
              </div>
            ))}
          </div>
          {focused && (
            <AgentDetails
              key={focused.key}
              session={session}
              row={focused}
              draft={drafts[focused.key] ?? ""}
              setDraft={(value) =>
                setDrafts((d) => ({ ...d, [focused.key]: value }))
              }
            />
          )}
        </>
      )}
    </section>
  );
}

function StopAll({
  session,
  fleets,
}: {
  session: string;
  fleets: Array<[string, FleetView]>;
}) {
  const stop = useStore((s) => s.stopFleet);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  if (!fleets.length) return null;
  return (
    <div className="agent-hub-stop-wrap">
      <button
        className="agent-hub-stop"
        aria-label="Stop all agents"
        title="Stop all agents in this chat"
        disabled={pending}
        onClick={async () => {
          setPending(true);
          setError("");
          const results = await Promise.allSettled(
            fleets.map(([id]) => stop(session, id)),
          );
          const failed = results.find((r) => r.status === "rejected");
          if (failed?.status === "rejected") {
            setError(String(failed.reason));
            setPending(false);
          }
        }}
      >
        {pending ? "Stopping…" : <Square size={11} />}
      </button>
      {error && (
        <span role="alert" className="agent-hub-error">
          {error}
        </span>
      )}
    </div>
  );
}

function Glyph({ status }: { status: string }) {
  if (status === "running")
    return <span className="agent-hub-dot" aria-label="working" />;
  if (status === "done") return <Check size={13} className="agent-hub-done" />;
  if (status === "failed") return <X size={13} className="agent-hub-failed" />;
  if (status === "cancelled") return <Square size={11} />;
  return <CircleDashed size={13} />;
}

function StopButton({ session, row }: { session: string; row: Row }) {
  const stop = useStore((s) => s.stopLane);
  const refresh = useStore((s) => s.refreshAgents);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  return (
    <div className="agent-hub-stop-wrap">
      <button
        className="agent-hub-stop"
        aria-label={`Stop ${row.label}`}
        title={error || `Stop ${row.label}`}
        disabled={pending}
        onClick={async () => {
          setPending(true);
          setError("");
          try {
            if (!(await stop(session, row.id))) {
              setPending(false);
              await refresh(session);
              setError("Agent already finished");
            }
          } catch (e) {
            setError(String(e));
            setPending(false);
          }
        }}
      >
        {pending ? <span>Stopping…</span> : <Square size={10} />}
      </button>
      {error && (
        <span role="alert" className="agent-hub-error">
          {error}
        </span>
      )}
    </div>
  );
}

function AgentDetails({
  session,
  row,
  draft,
  setDraft,
}: {
  session: string;
  row: Row;
  draft: string;
  setDraft: (text: string) => void;
}) {
  const openInspector = useStore((s) => s.openInspector);
  const watch = useStore((s) => s.watchLane);
  const steer = useStore((s) => s.steerLane);
  const refresh = useStore((s) => s.refreshAgents);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [patch, setPatch] = useState<string | null>(null);
  const [patchBusy, setPatchBusy] = useState(false);
  const [applied, setApplied] = useState(false);
  const running = active(row.status);
  useEffect(() => {
    setPatch(null);
    setApplied(false);
  }, [row.status, row.tokens]);
  return (
    <div className="agent-hub-details">
      <div className="agent-hub-meta">
        <code title={row.id}>{row.id.slice(0, 8)}</code>
        {row.model && <span>{row.model}</span>}
        {row.tokens > 0 && <span>{compactTokens(row.tokens)} tokens</span>}
        {row.elapsed_secs > 0 && <span>{row.elapsed_secs}s</span>}
        <span className="agent-hub-spacer" />
        {row.id && (
          <button
            onClick={() => (running ? watch(row.id) : openInspector(row.id))}
            aria-label={`Follow ${row.label}`}
          >
            <MessageSquare size={12} /> Transcript
          </button>
        )}
        {row.has_patch && !running && (
          <button
            disabled={patchBusy}
            onClick={async () => {
              setPatchBusy(true);
              setError("");
              try {
                setPatch(await agentPatch(session, row.id));
              } catch (e) {
                setError(String(e));
              } finally {
                setPatchBusy(false);
              }
            }}
          >
            <FileDiff size={12} /> Review changes
          </button>
        )}
      </div>
      {row.stop && <p className="agent-hub-note">{row.stop}</p>}
      <OutputTail text={row.tail || row.summary} />
      {patch !== null && (
        <div className="agent-hub-patch">
          <div>
            <span>Proposed changes</span>
            <button
              disabled={applied || patchBusy || !patch}
              onClick={async () => {
                setPatchBusy(true);
                setError("");
                try {
                  await applyAgentPatch(session, row.id, patch);
                  setApplied(true);
                } catch (e) {
                  setError(String(e));
                } finally {
                  setPatchBusy(false);
                }
              }}
            >
              {applied ? "Applied" : patchBusy ? "Applying…" : "Apply changes"}
            </button>
          </div>
          <pre aria-label="Agent changes">
            {patch || "No changes to apply."}
          </pre>
        </div>
      )}
      {row.id && (
        <form
          className="agent-hub-compose"
          onSubmit={async (event) => {
            event.preventDefault();
            const text = draft.trim();
            if (!text || pending) return;
            setPending(true);
            setError("");
            setNotice("");
            try {
              if (running) {
                if (!(await steer(session, row.id, text))) {
                  await refresh(session);
                  throw new Error(
                    "This agent has finished. Your message is saved; send it as a follow-up.",
                  );
                }
                setNotice("Queued for the agent’s next step");
              } else {
                await followUpAgent(session, row.id, text);
                await refresh(session);
                setNotice("Follow-up complete");
              }
              setDraft("");
            } catch (e) {
              setError(String(e).replace(/^Error: /, ""));
            } finally {
              setPending(false);
            }
          }}
        >
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            disabled={pending}
            aria-label={`${running ? "Steer" : "Follow up with"} ${row.label}`}
            placeholder={running ? "Add a direction…" : "Ask a follow-up…"}
          />
          <button disabled={pending || !draft.trim()} aria-label="Send message">
            <ArrowUp size={14} />
          </button>
        </form>
      )}
      {error && (
        <p role="alert" className="agent-hub-error">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="agent-hub-note">
          {notice}
        </p>
      )}
    </div>
  );
}

function OutputTail({ text }: { text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  const follow = useRef(true);
  const [paused, setPaused] = useState(false);
  useEffect(() => {
    if (follow.current && ref.current)
      ref.current.scrollTop = ref.current.scrollHeight;
  }, [text]);
  return (
    <div className="agent-hub-output">
      <pre
        ref={ref}
        onScroll={() => {
          const el = ref.current;
          if (!el) return;
          follow.current =
            el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          setPaused(!follow.current);
        }}
      >
        {text || "Waiting for output…"}
      </pre>
      {paused && (
        <button
          className="agent-hub-latest"
          onClick={() => {
            follow.current = true;
            setPaused(false);
            if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
          }}
        >
          <ArrowDown size={12} /> Latest output
        </button>
      )}
    </div>
  );
}
