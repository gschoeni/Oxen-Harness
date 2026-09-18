// The rows of a chat's agents hub: every subagent lane, running and finished,
// merged from two sources that share lane ids — the persisted records
// (`list_agents`) and the live fleet snapshots streaming in as events.
// Shared by the lane strip under the thread (FleetPanel) and the full agent
// view that opens from it (AgentView).

import type { FleetView } from "../../lib/store";
import type { AgentSummary } from "../../lib/types";

export type AgentRow = AgentSummary & {
  key: string;
  tail: string;
  activity: string;
  fleetIndex?: number;
};

/** Still going (or about to): the lane accepts directions, not follow-ups. */
export const isActive = (status: string) => status === "running" || status === "queued";

export const statusLabel = (status: string) =>
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
export function agentRows(agents: AgentSummary[], fleets: Array<[string, FleetView]>): AgentRow[] {
  const rows = new Map<string, AgentRow>();
  for (const agent of agents)
    rows.set(agent.id, {
      ...agent,
      key: agent.id,
      tail: "",
      activity: agent.summary,
    });
  for (const [fleetId, fleet] of fleets) {
    fleet.lanes.forEach((lane, index) => {
      const candidates = lane.id ? [] : agents.filter((a) => a.fleet === fleetId && a.label === lane.name);
      const saved = lane.id
        ? rows.get(lane.id)
        : candidates.length === 1 && fleet.lanes.filter((l) => l.name === lane.name).length === 1
          ? rows.get(candidates[0].id)
          : undefined;
      const key = lane.id || saved?.id || `${fleetId}:${index}`;
      if (saved && (fleet.finished || (saved.fleet === fleetId && !isActive(saved.status)))) return;
      rows.set(key, {
        ...saved,
        key,
        id: lane.id || saved?.id || "",
        label: lane.name,
        fleet: fleetId,
        status: fleet.finished && isActive(lane.status) ? "unknown" : lane.status,
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
  const sorted: AgentRow[] = [];
  const visited = new Set<string>();
  const visit = (row: AgentRow) => {
    if (visited.has(row.key)) return;
    visited.add(row.key);
    sorted.push(row);
    all.filter((r) => r.parent === row.id && r.id !== row.id).forEach(visit);
  };
  all.filter((row) => !row.parent || !rows.has(row.parent)).forEach(visit);
  all.forEach(visit);
  return sorted;
}
