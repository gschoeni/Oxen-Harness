// The lane strip under the thread: one row per subagent of this chat, running
// and finished, each a glance — glyph, name, what it's doing, its state. A
// row opens the agent in place of the thread column (see AgentView); nothing
// expands inline, so the strip stays a strip. Stops live here too: one per
// working row, and one for everything at the top.

import { useEffect, useState } from "react";
import { Check, ChevronDown, ChevronRight, CircleDashed, Square, Users, X } from "lucide-react";
import { compactTokens } from "../../lib/format";
import { fleetsFor, useStore, type FleetView } from "../../lib/store";
import { agentRows, isActive, statusLabel, type AgentRow } from "./agentRows";
import "./agents.css";

export { agentRows } from "./agentRows";

export function FleetPanel() {
  const session = useStore((s) => s.session?.session_id);
  return session ? <AgentHub key={session} session={session} /> : null;
}

function AgentHub({ session }: { session: string }) {
  const fleets = useStore((s) => s.fleets);
  const agents = useStore((s) => s.agents[session]);
  const refresh = useStore((s) => s.refreshAgents);
  const openAgent = useStore((s) => s.openAgent);
  const opened = useStore((s) => s.agentView[session]);
  const [open, setOpen] = useState(true);
  const mine = fleetsFor(fleets, session);
  const rows = agentRows(agents ?? [], mine);
  const knownIds = rows
    .filter((r) => isActive(r.status))
    .map((r) => r.id)
    .join(",");
  useEffect(() => {
    void refresh(session);
  }, [session, refresh, knownIds]);
  const working = rows.filter((r) => isActive(r.status)).length;
  const budget = mine.filter(([, f]) => !f.finished).slice(-1)[0]?.[1].budget;
  if (!rows.length) return null;
  return (
    <section className="agent-hub" aria-label="Agents">
      <div className="agent-hub-header">
        <button className="agent-hub-heading" onClick={() => setOpen(!open)} aria-expanded={open}>
          <Users size={14} />
          <strong>Agents</strong>
          <span>{working ? `${working} working` : `${rows.length} finished`}</span>
          {working > 0 && rows.length > working && <span>· {rows.length - working} finished</span>}
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
        <div className="agent-hub-rows">
          {rows.map((row) => (
            <div
              key={row.key}
              className={`agent-hub-row ${row.id && row.id === opened ? "selected" : ""}`}
            >
              <button
                className="agent-hub-select"
                style={{ paddingLeft: 14 + Math.min(row.depth ?? 0, 4) * 16 }}
                title={row.id ? `Open ${row.label}` : `${row.label} is waiting for a slot`}
                disabled={!row.id}
                onClick={() => row.id && openAgent(session, row.id)}
              >
                <Glyph status={row.status} />
                <span className="agent-hub-name">{row.label}</span>
                <span className="agent-hub-activity">{row.activity || statusLabel(row.status)}</span>
                <span className={`agent-hub-status ${row.status}`}>{statusLabel(row.status)}</span>
                <ChevronRight size={13} className="agent-hub-open" aria-hidden="true" />
              </button>
              {isActive(row.status) && row.id && <StopButton session={session} row={row} />}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function StopAll({ session, fleets }: { session: string; fleets: Array<[string, FleetView]> }) {
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
          const results = await Promise.allSettled(fleets.map(([id]) => stop(session, id)));
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
  if (status === "running") return <span className="agent-hub-dot" aria-label="working" />;
  if (status === "done") return <Check size={13} className="agent-hub-done" />;
  if (status === "failed") return <X size={13} className="agent-hub-failed" />;
  if (status === "cancelled") return <Square size={11} />;
  return <CircleDashed size={13} />;
}

function StopButton({ session, row }: { session: string; row: AgentRow }) {
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
