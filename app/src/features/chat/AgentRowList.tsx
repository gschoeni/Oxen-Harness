// The rows of subagent lanes, shared by the spawn card in the thread (where a
// fleet's lanes live for good, beside the call that started them) and the
// strip of agents still working under the thread. A row is a glance — glyph,
// name, what it's doing, its state — that opens the agent in place of the
// thread column; a working row carries its stop.

import { useState, type CSSProperties } from "react";
import { Check, ChevronRight, CircleDashed, Square, X } from "lucide-react";
import { useStore } from "../../lib/store";
import { isActive, statusLabel, type AgentRow } from "./agentRows";
import "./agents.css";

export function AgentRowList({ session, rows }: { session: string; rows: AgentRow[] }) {
  const openAgent = useStore((s) => s.openAgent);
  const opened = useStore((s) => s.agentView[session]);
  return (
    <div className="agent-hub-rows">
      {rows.map((row) => (
        <div key={row.key} className={`agent-hub-row ${row.id && row.id === opened ? "selected" : ""}`}>
          <button
            className="agent-hub-select"
            style={{ "--depth": Math.min(row.depth ?? 0, 4) } as CSSProperties}
            title={row.id ? `Open ${row.label}` : `${row.label} is waiting for a slot`}
            disabled={!row.id}
            onClick={() => row.id && openAgent(session, row.id)}
          >
            <Glyph status={row.status} />
            <span className="agent-hub-name">{row.label}</span>
            <span className="agent-hub-activity">{row.activity}</span>
            <span className={`agent-hub-status ${row.status}`}>{statusLabel(row.status)}</span>
            <ChevronRight size={13} className="agent-hub-open" aria-hidden="true" />
          </button>
          {isActive(row.status) && row.id && <StopButton session={session} row={row} />}
        </div>
      ))}
    </div>
  );
}

/** The lane ids a spawn result names ("agent id: …"): how a resumed chat,
 *  whose fleets are long gone, still knows which saved agents were this
 *  call's. */
export function agentIdsInResult(result: string): string[] {
  return Array.from(result.matchAll(/agent id: (\S+)/g), (m) => m[1]);
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
