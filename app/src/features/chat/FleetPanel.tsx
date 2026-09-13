// The fleet panel: live lanes for N parallel subagents running in this chat —
// a review fan-out step or a `spawn_agents` call the model made mid-turn.
// Each lane shows its status, name, a one-line activity readout, and token
// spend; clicking a lane expands it to watch that agent's live output tail
// (click again, or another lane, to switch). A fleet can be stopped on its
// own — the turn around it carries on with the partial report. The panel
// appears when a fleet starts and disappears when it finishes — results land
// in the thread. A background (`wait: false`) fleet can overlap a later one,
// so a chat may show more than one panel, oldest first.

import { useEffect, useRef } from "react";
import { Check, CircleDashed, Square, Users, X } from "lucide-react";
import { compactTokens } from "../../lib/format";
import { fleetsFor, useStore, type FleetLane, type FleetView } from "../../lib/store";

export function FleetPanel() {
  const sessionId = useStore((s) => s.session?.session_id);
  const fleets = useStore((s) => s.fleets);
  if (!sessionId) return null;
  const mine = fleetsFor(fleets, sessionId);
  if (mine.length === 0) return null;
  return (
    <>
      {mine.map(([id, fleet]) => (
        <OneFleet key={id} id={id} fleet={fleet} />
      ))}
    </>
  );
}

function OneFleet({ id, fleet }: { id: string; fleet: FleetView }) {
  const setFocus = useStore((s) => s.setFleetFocus);
  const stopFleet = useStore((s) => s.stopFleet);
  const running = fleet.lanes.filter((l) => l.status === "running").length;
  const settled = fleet.lanes.every((l) => l.status === "done" || l.status === "failed");
  const focused = fleet.focused !== null ? fleet.lanes[fleet.focused] : null;

  return (
    <div className="fleet-panel" role="status" aria-label="Parallel agents">
      <div className="fleet-panel-head">
        <Users size={13} className="fleet-panel-icon" />
        <span className="fleet-panel-title">
          {fleet.source === "review" ? "Review agents" : "Agents"} — {running} of{" "}
          {fleet.lanes.length} running
        </span>
        <span className="fleet-panel-hint">
          {focused ? "click again to collapse" : "click a lane to watch it"}
        </span>
        {!settled && (
          <button
            type="button"
            className="fleet-panel-stop"
            onClick={() => stopFleet(fleet.session, id)}
            title="Stop these agents (the chat keeps going with what they have)"
            aria-label="Stop agents"
          >
            <Square size={10} />
            Stop
          </button>
        )}
      </div>
      <div className="fleet-lanes">
        {fleet.lanes.map((lane, i) => (
          <button
            key={`${lane.name}-${i}`}
            className={`fleet-lane ${fleet.focused === i ? "focused" : ""}`}
            onClick={() => setFocus(id, fleet.focused === i ? null : i)}
            aria-pressed={fleet.focused === i}
            title={`Watch ${lane.name}`}
          >
            <LaneGlyph lane={lane} />
            <span className="fleet-lane-name">{lane.name}</span>
            <span className="fleet-lane-activity">{lane.activity}</span>
            {lane.tokens > 0 && (
              <span className="fleet-lane-tokens">{compactTokens(lane.tokens)}</span>
            )}
          </button>
        ))}
      </div>
      {focused && <LaneTail tail={focused.tail} />}
    </div>
  );
}

function LaneGlyph({ lane }: { lane: FleetLane }) {
  switch (lane.status) {
    case "queued":
      return <CircleDashed size={12} className="fleet-glyph queued" />;
    case "running":
      return <span className="fleet-glyph running" aria-label="running" />;
    case "done":
      return <Check size={12} className="fleet-glyph done" />;
    case "failed":
      return <X size={12} className="fleet-glyph failed" />;
  }
}

/** The expanded lane's live output, auto-scrolled to the newest text. */
function LaneTail({ tail }: { tail: string }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [tail]);
  return (
    <pre className="fleet-tail" ref={ref}>
      {tail || "…waiting for output"}
    </pre>
  );
}
