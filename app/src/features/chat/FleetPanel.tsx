// The strip of agents still working, pinned under the thread so a running
// fleet's state, spend, and stops are in reach while it runs. It steps aside
// once every agent has finished: a fleet's lanes live for good in the thread,
// inside the spawn card that started them (see AgentRowList / ToolCall).

import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, Square, Users } from "lucide-react";
import { compactTokens, formatUsd } from "../../lib/format";
import { fleetsFor, useStore, type FleetView } from "../../lib/store";
import { agentRows, isActive } from "./agentRows";
import { AgentRowList } from "./AgentRowList";
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
  const spend = useStore((s) => s.treeUsage[session]);
  if (!working) return null;
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
          {spend?.cost != null && (
            <span className="agent-hub-budget" title="This chat's spend so far, agents included">
              {formatUsd(spend.cost)}
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
      {open && <AgentRowList session={session} rows={rows} />}
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
