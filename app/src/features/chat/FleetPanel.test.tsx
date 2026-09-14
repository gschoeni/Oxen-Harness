import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { agentRows, FleetPanel } from "./FleetPanel";
import { useStore } from "../../lib/store";
import type { AgentSummary } from "../../lib/types";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
const saved = (
  id = "lane-0",
  extra: Partial<AgentSummary> = {},
): AgentSummary => ({
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
    s.ingestFleetStarted({
      session: "s1",
      fleet,
      agents: names,
      source: "review",
    });
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
describe("agent hub", () => {
  it("stays quiet until there are agents, then combines overlapping fleets", async () => {
    const { container } = render(<FleetPanel />);
    expect(container).toBeEmptyDOMElement();
    await start();
    await start("f2", ["explore"]);
    expect(screen.getAllByRole("region", { name: "Agents" })).toHaveLength(1);
    expect(screen.getByText("3 working")).toBeInTheDocument();
    expect(screen.getByText("explore")).toBeInTheDocument();
  });
  it("shows durable results after completion without duplicate rows", async () => {
    render(<FleetPanel />);
    await start("f1", ["diff-scan"]);
    ipc.listAgents.mockResolvedValue([saved("f1-0")]);
    await act(async () => useStore.getState().ingestFleetCompleted("s1", "f1"));
    expect(await screen.findByText("4 candidates")).toBeInTheDocument();
    expect(screen.getAllByText("diff-scan")).toHaveLength(1);
    expect(screen.getByText("1 finished")).toBeInTheDocument();
    await userEvent.click(screen.getByTitle("Watch diff-scan"));
    await userEvent.click(
      screen.getByRole("button", { name: "Follow diff-scan" }),
    );
    expect(useStore.getState().inspector?.sessionId).toBe("f1-0");
  });
  it("retains steering drafts after failure and across selection", async () => {
    render(<FleetPanel />);
    await start();
    await userEvent.click(screen.getByTitle("Watch diff-scan"));
    ipc.interjectAgent.mockRejectedValueOnce(new Error("Connection lost"));
    const box = screen.getByRole("textbox", { name: "Steer diff-scan" });
    await userEvent.type(box, "check the tests{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Connection lost",
    );
    expect(box).toHaveValue("check the tests");
    await userEvent.click(screen.getByTitle("Watch callers"));
    await userEvent.click(screen.getByTitle("Watch diff-scan"));
    expect(
      screen.getByRole("textbox", { name: "Steer diff-scan" }),
    ).toHaveValue("check the tests");
    await userEvent.click(screen.getByRole("button", { name: "Send message" }));
    expect(ipc.interjectAgent).toHaveBeenLastCalledWith(
      "s1",
      "f1-0",
      "check the tests",
    );
    expect(
      screen.getByRole("textbox", { name: "Steer diff-scan" }),
    ).toHaveValue("");
  });
  it("keeps a message if the agent finishes before delivery", async () => {
    render(<FleetPanel />);
    await start();
    ipc.interjectAgent.mockResolvedValueOnce(false);
    await userEvent.click(screen.getByTitle("Watch diff-scan"));
    const box = screen.getByRole("textbox", { name: "Steer diff-scan" });
    await userEvent.type(box, "another check{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Your message is saved",
    );
    expect(box).toHaveValue("another check");
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
    expect(
      screen.queryByRole("button", { name: "Stop callers" }),
    ).not.toBeInTheDocument();
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
  it("reviews the exact patch before applying and retains it after conflicts", async () => {
    ipc.listAgents.mockResolvedValue([saved("lane-0", { has_patch: true })]);
    ipc.agentPatch.mockResolvedValue("diff --git a/test b/test\n+fixed\n");
    render(<FleetPanel />);
    await userEvent.click(await screen.findByTitle("Watch diff-scan"));
    expect(
      screen.queryByRole("button", { name: "Apply changes" }),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Review changes" }),
    );
    expect(screen.getByLabelText("Agent changes")).toHaveTextContent("+fixed");
    ipc.applyAgentPatch.mockRejectedValueOnce(
      new Error("Patch conflicts with local edits"),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Apply changes" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Patch conflicts",
    );
    expect(ipc.applyAgentPatch).toHaveBeenCalledWith(
      "s1",
      "lane-0",
      "diff --git a/test b/test\n+fixed\n",
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Apply changes" }),
    );
    expect(screen.getByRole("button", { name: "Applied" })).toBeDisabled();
  });
  it("continues a finished agent with a follow-up", async () => {
    ipc.listAgents.mockResolvedValue([saved()]);
    render(<FleetPanel />);
    await userEvent.click(await screen.findByTitle("Watch diff-scan"));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Follow up with diff-scan" }),
      "fix candidate 2{Enter}",
    );
    expect(ipc.followUpAgent).toHaveBeenCalledWith(
      "s1",
      "lane-0",
      "fix candidate 2",
    );
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Follow-up complete",
    );
  });
  it("reconciles queued placeholders with history and honors persisted completion", () => {
    const agent = saved("durable-id", { status: "running" });
    const fleet = {
      session: "s1",
      source: "turn" as const,
      focused: null,
      lanes: [
        {
          id: "",
          name: "diff-scan",
          status: "queued" as const,
          activity: "",
          tail: "",
          tokens: 0,
        },
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
