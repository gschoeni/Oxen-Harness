import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
import { AgentView } from "./AgentView";
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
  model: "qwen3-8-max",
  ...extra,
});

const transcript = [
  { role: "system", content: "You are a subagent." },
  { role: "user", content: "Find the naming conventions in the six model files." },
  { role: "assistant", content: "They all agree on snake_case keys." },
];

async function start(fleet = "f1", names = ["diff-scan", "callers"]) {
  await act(async () => {
    const s = useStore.getState();
    s.ingestFleetStarted({ session: "s1", fleet, agents: names, source: "turn" });
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
    agentView: { s1: "lane-0" },
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
  });
  ipc.sessionMessages.mockResolvedValue(transcript);
});

describe("the agent view", () => {
  it("leaves through one chevron button, with no text to mistake for a breadcrumb", async () => {
    ipc.listAgents.mockResolvedValue([saved()]);
    render(<AgentView session="s1" lane="lane-0" />);
    const back = await screen.findByRole("button", { name: "Back to the chat" });
    expect(back).toHaveTextContent("");
    expect(back).toHaveClass("icon-btn");
    fireEvent.click(back);
    expect(useStore.getState().agentView.s1).toBeNull();
  });

  it("shows the agent's transcript with its brief as an assignment, not a bubble", async () => {
    ipc.listAgents.mockResolvedValue([saved()]);
    render(<AgentView session="s1" lane="lane-0" />);
    expect(await screen.findByText("Assignment")).toBeInTheDocument();
    expect(screen.getByText("Find the naming conventions in the six model files.")).toBeInTheDocument();
    expect(await screen.findByText("They all agree on snake_case keys.")).toBeInTheDocument();
    expect(screen.queryByText("You are a subagent.")).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "diff-scan" })).toBeInTheDocument();
    expect(screen.getByText("Done")).toBeInTheDocument();
    expect(screen.getByText("qwen3-8-max")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Follow up with diff-scan…")).toBeInTheDocument();
  });

  it("gives a working agent a direction and shows it queued until the transcript has it", async () => {
    await start();
    render(<AgentView session="s1" lane="f1-0" />);
    const box = await screen.findByPlaceholderText(/Give diff-scan a direction/);
    await userEvent.type(box, "check the tests too{Enter}");
    expect(ipc.interjectAgent).toHaveBeenCalledWith("s1", "f1-0", "check the tests too");
    expect(screen.getByText("check the tests too")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Queued for its next step");
    // The transcript catches up: the bubble is the persisted one now.
    ipc.sessionMessages.mockResolvedValue([...transcript, { role: "user", content: "check the tests too" }]);
    await act(async () =>
      useStore.getState().ingestFleetAgent({
        session: "s1",
        fleet: "f1",
        agent: 0,
        lane: "f1-0",
        name: "diff-scan",
        phase: "done",
        tokens: 9,
        summary: "ok",
      }),
    );
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    expect(screen.getAllByText("check the tests too")).toHaveLength(1);
  });

  it("offers a follow-up when the agent finished before the direction landed", async () => {
    await start();
    ipc.interjectAgent.mockResolvedValueOnce(false);
    render(<AgentView session="s1" lane="f1-0" />);
    await userEvent.type(await screen.findByPlaceholderText(/Give diff-scan/), "another check{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("finished before your message arrived");
    await userEvent.click(screen.getByRole("button", { name: "Send as a follow-up" }));
    expect(ipc.followUpAgent).toHaveBeenCalledWith("s1", "f1-0", "another check");
  });

  it("follows up a finished agent and shows the message while it sends", async () => {
    ipc.listAgents.mockResolvedValue([saved()]);
    let finish: (v: string) => void = () => {};
    ipc.followUpAgent.mockImplementationOnce(() => new Promise<string>((r) => (finish = r)));
    render(<AgentView session="s1" lane="lane-0" />);
    await userEvent.type(await screen.findByPlaceholderText("Follow up with diff-scan…"), "fix candidate 2{Enter}");
    expect(ipc.followUpAgent).toHaveBeenCalledWith("s1", "lane-0", "fix candidate 2");
    expect(screen.getByText("fix candidate 2")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Sending…");
    expect(screen.getByRole("button", { name: "Stop generating" })).toBeInTheDocument();
    ipc.listAgents.mockClear();
    await act(async () => finish("done"));
    await waitFor(() => expect(ipc.listAgents).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "Stop generating" })).not.toBeInTheDocument();
  });

  it("reviews the exact patch before applying and keeps it after a conflict", async () => {
    ipc.listAgents.mockResolvedValue([saved("lane-0", { has_patch: true })]);
    ipc.agentPatch.mockResolvedValue("diff --git a/test b/test\n+fixed\n");
    render(<AgentView session="s1" lane="lane-0" />);
    await userEvent.click(await screen.findByRole("button", { name: "Changes" }));
    expect(screen.getByLabelText("Agent changes")).toHaveTextContent("+fixed");
    ipc.applyAgentPatch.mockRejectedValueOnce(new Error("Patch conflicts with local edits"));
    await userEvent.click(screen.getByRole("button", { name: "Apply changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Patch conflicts");
    expect(ipc.applyAgentPatch).toHaveBeenCalledWith("s1", "lane-0", "diff --git a/test b/test\n+fixed\n");
    await userEvent.click(screen.getByRole("button", { name: "Apply changes" }));
    expect(screen.getByRole("button", { name: "Applied" })).toBeDisabled();
  });

  it("stops a working agent from the bar", async () => {
    await start();
    render(<AgentView session="s1" lane="f1-0" />);
    await userEvent.click(await screen.findByRole("button", { name: "Stop diff-scan" }));
    expect(ipc.cancelAgent).toHaveBeenCalledWith("s1", "f1-0");
    expect(screen.getByText("Stopping…")).toBeInTheDocument();
  });

  it("steps between sibling agents and returns to the chat on Esc", async () => {
    ipc.listAgents.mockResolvedValue([saved("lane-0"), saved("lane-1", { label: "callers", created_at: 2 })]);
    render(<AgentView session="s1" lane="lane-0" />);
    expect(await screen.findByText("1 / 2")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous agent" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Next agent" }));
    expect(useStore.getState().agentView.s1).toBe("lane-1");
    fireEvent.keyDown(screen.getByRole("region", { name: "Agent diff-scan" }), { key: "Escape" });
    expect(useStore.getState().agentView.s1).toBeNull();
  });
});
