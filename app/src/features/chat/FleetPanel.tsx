// The strip of agents still working that have no card in the thread — a
// review's fan-out, started by the host rather than by a model's call —
// pinned under the thread so their state, spend, and stops are in reach. A
// fleet a model's call started lives in that call's card instead (see
// AgentRowList / ToolCall), and the strip steps aside when nothing of its
// own is working.

import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, Users } from "lucide-react";
import { compactTokens, formatUsd } from "../../lib/format";
import { fleetsFor, useStore } from "../../lib/store";
import { agentRows, isActive } from "./agentRows";
import { AgentRowList, StopFleets } from "./AgentRowList";
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
  // Fleets a model's call started have a card in the thread that carries
  // their lanes; the strip is for the rest (a review's fan-out).
  const mine = fleetsFor(fleets, session).filter(([, f]) => !f.call);
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
          <StopFleets
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

