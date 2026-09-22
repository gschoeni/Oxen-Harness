import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { FleetPanel } from "./FleetPanel";
import { agentRows } from "./agentRows";
import { useStore } from "../../lib/store";
import type { AgentSummary } from "../../lib/types";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const saved = (id = "lane-0", extra: Partial<AgentSummary> = {}): AgentSummary => ({
  id,
  label: "diff-scan",
  fleet: "f1",
  status: "done",
  summary: "4 candidates",
  tokens: 1200,
  rounds: 2,
  elapsed_secs: 8,
  created_at: 1,
  ...extra,
});

async function start(fleet = "f1", names = ["diff-scan", "callers"]) {
  await act(async () => {
    const s = useStore.getState();
    s.ingestFleetStarted({ session: "s1", fleet, agents: names, source: "review" });
    names.forEach((name, agent) =>
      s.ingestFleetAgent({
        session: "s1",
        fleet,
        agent,
        lane: `${fleet}-${agent}`,
        name,
        phase: "started",
        tokens: 0,
        summary: "",
      }),
    );
  });
}

beforeEach(() => {
  resetAll();
  useStore.setState({
    agents: {},
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
  });
});

describe("the lane strip", () => {
  it("stays quiet until there are agents, then combines overlapping fleets", async () => {
    const { container } = render(<FleetPanel />);
    expect(container).toBeEmptyDOMElement();
    await start();
    await start("f2", ["explore"]);
    expect(screen.getAllByRole("region", { name: "Agents" })).toHaveLength(1);
    expect(screen.getByText("3 working")).toBeInTheDocument();
    expect(screen.getByText("explore")).toBeInTheDocument();
  });

  it("steps aside once every agent has finished — the rows live in the spawn card", async () => {
    const { container } = render(<FleetPanel />);
    await start("f1", ["diff-scan"]);
    ipc.listAgents.mockResolvedValue([saved("f1-0")]);
    await act(async () => useStore.getState().ingestFleetCompleted("s1", "f1"));
    await act(async () => {});
    expect(container).toBeEmptyDOMElement();
  });

  it("says a row's state once when it has no activity to show", async () => {
    render(<FleetPanel />);
    await start("f1", ["diff-scan"]);
    const row = screen.getByTitle("Open diff-scan");
    expect(row.querySelector(".agent-hub-activity")).toBeEmptyDOMElement();
    expect(row.querySelector(".agent-hub-status")).toHaveTextContent("Working");
  });

  it("opens an agent in the thread column and marks the open row", async () => {
    render(<FleetPanel />);
    await start();
    await userEvent.click(screen.getByTitle("Open diff-scan"));
    expect(useStore.getState().agentView.s1).toBe("f1-0");
    expect(screen.getByTitle("Open diff-scan").closest(".agent-hub-row")).toHaveClass("selected");
    expect(screen.getByTitle("Open callers").closest(".agent-hub-row")).not.toHaveClass("selected");
  });

  it("cannot open an agent still waiting for a slot", async () => {
    await act(async () => {
      useStore
        .getState()
        .ingestFleetStarted({ session: "s1", fleet: "f1", agents: ["diff-scan"], source: "turn" });
    });
    render(<FleetPanel />);
    const row = await screen.findByTitle("diff-scan is waiting for a slot");
    expect(row).toBeDisabled();
    await userEvent.click(row);
    expect(useStore.getState().agentView.s1).toBeUndefined();
    // Let the strip's refresh of the hub land before the test ends.
    await act(async () => {});
  });

  it("acknowledges stopping until the terminal event arrives", async () => {
    render(<FleetPanel />);
    await start();
    const stop = screen.getByRole("button", { name: "Stop callers" });
    await userEvent.click(stop);
    expect(ipc.cancelAgent).toHaveBeenCalledWith("s1", "f1-1");
    expect(stop).toBeDisabled();
    expect(stop).toHaveTextContent("Stopping…");
    await act(async () =>
      useStore.getState().ingestFleetAgent({
        session: "s1",
        fleet: "f1",
        agent: 1,
        lane: "f1-1",
        name: "callers",
        phase: "cancelled",
        tokens: 4,
        summary: "Partial answer",
      }),
    );
    expect(screen.getByText("Stopped")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop callers" })).not.toBeInTheDocument();
  });

  it("reports stop errors and permits retry", async () => {
    render(<FleetPanel />);
    await start();
    ipc.cancelAgent.mockRejectedValueOnce(new Error("Offline"));
    const stop = screen.getByRole("button", { name: "Stop callers" });
    await userEvent.click(stop);
    expect(await screen.findByRole("alert")).toHaveTextContent("Offline");
    expect(stop).toBeEnabled();
  });

  it("reconciles queued placeholders with history and honors persisted completion", () => {
    const agent = saved("durable-id", { status: "running" });
    const fleet = {
      session: "s1",
      source: "turn" as const,
      focused: null,
      lanes: [
        { id: "", name: "diff-scan", status: "queued" as const, activity: "", tail: "", tokens: 0 },
      ],
    };
    const rows = agentRows([agent], [["f1", fleet]]);
    expect(rows).toHaveLength(1);
    expect(rows[0].id).toBe("durable-id");
    expect(rows[0].status).toBe("queued");
    fleet.lanes[0].id = "durable-id";
    const finished = agentRows([saved("durable-id")], [["f1", fleet]]);
    expect(finished[0].status).toBe("done");
  });

  it("orders descendants and tolerates cyclic ancestry", () => {
    const rows = agentRows(
      [
        saved("child", { parent: "parent" }),
        saved("parent"),
        saved("a", { parent: "b" }),
        saved("b", { parent: "a" }),
      ],
      [],
    );
    expect(rows.map((r) => r.id)).toEqual(["parent", "child", "a", "b"]);
  });
});
