import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ToolCall } from "./ToolCall";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { AgentSummary } from "../../lib/types";
import type { Item } from "./thread";

type ToolItem = Extract<Item, { kind: "tool" }>;

const spawn = (over: Partial<ToolItem>): ToolItem =>
  ({
    kind: "tool",
    id: "t1",
    name: "spawn_agents",
    callId: "call_9",
    args: JSON.stringify({ agents: [{ name: "diff-scan", prompt: "go" }] }),
    result: "",
    running: true,
    startedAt: Date.now(),
    ...over,
  }) as ToolItem;

const saved = (id: string, label: string, extra: Partial<AgentSummary> = {}): AgentSummary => ({
  id,
  label,
  fleet: "f1",
  status: "done",
  summary: "4 candidates",
  tokens: 1200,
  rounds: 2,
  elapsed_secs: 8,
  created_at: 1,
  ...extra,
});

beforeEach(() => {
  resetAll();
  useStore.setState({
    agents: {},
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
  });
});

async function startFleet(fleet: string, call: string | undefined, names: string[]) {
  await act(async () => {
    const s = useStore.getState();
    s.ingestFleetStarted({ session: "s1", fleet, agents: names, source: "turn", call });
    names.forEach((name, agent) =>
      s.ingestFleetAgent({ session: "s1", fleet, agent, lane: `${fleet}-${agent}`, name, phase: "started", tokens: 0, summary: "" }),
    );
  });
}

describe("a spawn card in the thread", () => {
  it("shows the lanes of the fleet its call started, and only those, while they run", async () => {
    await startFleet("f1", "call_9", ["diff-scan", "callers"]);
    await startFleet("f2", "call_other", ["explore"]);
    render(<ToolCall item={spawn({})} />);
    expect(screen.getByText("diff-scan")).toBeInTheDocument();
    expect(screen.getByText("callers")).toBeInTheDocument();
    expect(screen.queryByText("explore")).not.toBeInTheDocument();
    expect(screen.getAllByText("Working")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Stop callers" })).toBeInTheDocument();
  });

  it("keeps the lanes its result names once everything is done, even in a resumed chat", async () => {
    // No live fleet at all: the chat was reopened. The result names the lanes.
    ipc.listAgents.mockResolvedValue([saved("lane-a", "diff-scan"), saved("lane-b", "other-call")]);
    render(
      <ToolCall
        item={spawn({
          running: false,
          endedAt: Date.now(),
          result: "### diff-scan — done · 1.2k tok · 2 rounds\nagent id: lane-a\n\n4 candidates",
        })}
      />,
    );
    expect(await screen.findByText("diff-scan")).toBeInTheDocument();
    expect(screen.queryByText("other-call")).not.toBeInTheDocument();
    expect(screen.getByText("Done")).toBeInTheDocument();
    await userEvent.click(screen.getByTitle("Open diff-scan"));
    expect(useStore.getState().agentView.s1).toBe("lane-a");
  });

  it("carries a lane's descendants with it", async () => {
    useStore.setState({
      agents: {
        s1: [saved("lane-a", "diff-scan"), saved("lane-a1", "leaf", { parent: "lane-a", depth: 1 })],
      },
    });
    render(<ToolCall item={spawn({ running: false, result: "agent id: lane-a" })} />);
    expect(screen.getByText("diff-scan")).toBeInTheDocument();
    expect(screen.getByText("leaf")).toBeInTheDocument();
  });
});
