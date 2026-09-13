import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { FleetPanel } from "./FleetPanel";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const started = (fleet = "f1", agents = ["diff-scan", "callers"]) =>
  act(() =>
    useStore.getState().ingestFleetStarted({
      session: "s1",
      fleet,
      agents,
      source: "review",
    }),
  );

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    infos: { s1: { ...ipc.sampleSession, session_id: "s1" } },
  });
});

describe("FleetPanel", () => {
  it("renders nothing until a fleet starts, then one lane per agent", () => {
    const { container } = render(<FleetPanel />);
    expect(container).toBeEmptyDOMElement();

    started();
    expect(screen.getByText(/Review agents — 0 of 2 running/)).toBeInTheDocument();
    expect(screen.getByText("diff-scan")).toBeInTheDocument();
    expect(screen.getByText("callers")).toBeInTheDocument();
  });

  it("tracks lane state, activity, and tokens from fleet events", () => {
    render(<FleetPanel />);
    started();
    act(() => {
      const s = useStore.getState();
      s.ingestFleetAgent({
        session: "s1",
        fleet: "f1",
        agent: 0,
        lane: "lane-0",
        name: "diff-scan",
        phase: "started",
        tokens: 0,
        summary: "",
      });
      s.ingestFleetActivity({
        session: "s1",
        fleet: "f1",
        agent: 0,
        kind: "token",
        text: "reading the parser",
        tokens: null,
      });
      s.ingestFleetActivity({
        session: "s1",
        fleet: "f1",
        agent: 0,
        kind: "tokens",
        text: "",
        tokens: 12_300,
      });
    });
    expect(screen.getByText(/1 of 2 running/)).toBeInTheDocument();
    expect(screen.getByText("reading the parser")).toBeInTheDocument();
    expect(screen.getByText("12.3k")).toBeInTheDocument();

    act(() =>
      useStore.getState().ingestFleetAgent({
        session: "s1",
        fleet: "f1",
        agent: 0,
        lane: "lane-0",
        name: "diff-scan",
        phase: "done",
        tokens: 15_000,
        summary: "4 candidates",
      }),
    );
    expect(screen.getByText(/0 of 2 running/)).toBeInTheDocument();
    expect(screen.getByText("4 candidates")).toBeInTheDocument();
  });

  it("shows where the turn's tree budget stands once a fleet reports it", () => {
    render(<FleetPanel />);
    started();
    expect(screen.queryByText(/tree /)).not.toBeInTheDocument();
    act(() =>
      useStore.getState().ingestFleetBudget({
        session: "s1",
        fleet: "f1",
        tokens: 42_000,
        max_tokens: 1_500_000,
        requests: 7,
        max_requests: 200,
        spawns: 3,
        max_spawns: 24,
      }),
    );
    expect(screen.getByText(/tree 42\.0k \/ 1\.5M · 3\/24 agents/)).toBeInTheDocument();
  });

  it("clicking a lane expands its live output tail; clicking again collapses", async () => {
    render(<FleetPanel />);
    started();
    act(() =>
      useStore.getState().ingestFleetActivity({
        session: "s1",
        fleet: "f1",
        agent: 1,
        kind: "token",
        text: "tracing call sites across the repo",
        tokens: null,
      }),
    );

    const lane = screen.getByRole("button", { name: /callers/ });
    await userEvent.click(lane);
    // The tail pane shows the lane's streamed output.
    expect(screen.getAllByText(/tracing call sites/).length).toBeGreaterThan(1);
    expect(screen.getByText(/click again to collapse/)).toBeInTheDocument();

    await userEvent.click(lane);
    expect(screen.getByText(/click a lane to watch it/)).toBeInTheDocument();
  });

  it("the panel closes when the fleet completes", () => {
    const { container } = render(<FleetPanel />);
    started();
    expect(screen.getByText("diff-scan")).toBeInTheDocument();
    act(() => useStore.getState().ingestFleetCompleted("s1", "f1"));
    expect(container).toBeEmptyDOMElement();
  });

  it("two fleets overlapping in one chat keep separate lanes and panels", () => {
    render(<FleetPanel />);
    started("f1", ["diff-scan", "callers"]);
    started("f2", ["explore"]);
    // Lane 0 of the second fleet is not lane 0 of the first.
    act(() =>
      useStore.getState().ingestFleetActivity({
        session: "s1",
        fleet: "f2",
        agent: 0,
        kind: "token",
        text: "walking the crate tree",
        tokens: null,
      }),
    );
    expect(screen.getAllByRole("status")).toHaveLength(2);
    expect(screen.getByText("walking the crate tree")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /diff-scan/ })).not.toHaveTextContent(
      "walking the crate tree",
    );
    // The first finishing closes only its own panel.
    act(() => useStore.getState().ingestFleetCompleted("s1", "f1"));
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByText("explore")).toBeInTheDocument();
  });

  it("a running lane can be followed live and steered", async () => {
    render(<FleetPanel />);
    started("f1");
    act(() =>
      useStore.getState().ingestFleetAgent({
        session: "s1",
        fleet: "f1",
        agent: 0,
        lane: "lane-0",
        name: "diff-scan",
        phase: "started",
        tokens: 0,
        summary: "",
      }),
    );
    // Follow opens the inspector on the lane, live.
    await userEvent.click(screen.getByRole("button", { name: "Follow diff-scan" }));
    expect(useStore.getState().inspector?.sessionId).toBe("lane-0");
    expect(useStore.getState().inspectorLive).toBe(true);
    // Watching the lane shows a steer box; Enter sends the line to that lane.
    await userEvent.click(screen.getByTitle("Watch diff-scan"));
    const box = screen.getByRole("textbox", { name: "Steer diff-scan" });
    await userEvent.type(box, "check the tests too{Enter}");
    expect(ipc.interjectAgent).toHaveBeenCalledWith("s1", "lane-0", "check the tests too");
    expect(box).toHaveValue("");
  });

  it("a running lane can be stopped on its own", async () => {
    render(<FleetPanel />);
    started("f1");
    act(() =>
      useStore.getState().ingestFleetAgent({
        session: "s1",
        fleet: "f1",
        agent: 1,
        lane: "lane-1",
        name: "callers",
        phase: "started",
        tokens: 0,
        summary: "",
      }),
    );
    // Only the running lane (which has an id) offers a stop.
    expect(screen.queryByRole("button", { name: "Stop diff-scan" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Stop callers" }));
    expect(ipc.cancelAgent).toHaveBeenCalledWith("s1", "lane-1");
  });

  it("finished agents are listed in a collapsed hub once a fleet ends", async () => {
    ipc.listAgents.mockResolvedValue([
      {
        id: "lane-1",
        label: "diff-scan",
        fleet: "f1",
        status: "done",
        summary: "4 candidates",
        tokens: 12_300,
        rounds: 3,
        elapsed_secs: 0,
        created_at: 1,
      },
      {
        id: "lane-2",
        label: "callers",
        fleet: "f1",
        status: "failed",
        summary: "rate limited",
        tokens: 0,
        rounds: 0,
        elapsed_secs: 0,
        created_at: 2,
      },
    ]);
    render(<FleetPanel />);
    started("f1");
    act(() => useStore.getState().ingestFleetCompleted("s1", "f1"));
    await vi.waitFor(() => {
      expect(screen.getByText("2 finished agents")).toBeInTheDocument();
    });
    expect(ipc.listAgents).toHaveBeenCalledWith("s1");
    // Collapsed until opened; then one row per lane with its verdict.
    expect(screen.queryByText("4 candidates")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /finished agents/ }));
    expect(screen.getByText("4 candidates")).toBeInTheDocument();
    expect(screen.getByText(/failed — rate limited/)).toBeInTheDocument();
    // A row opens that lane's transcript in the inspector.
    await userEvent.click(screen.getByTitle("Open diff-scan's transcript"));
    expect(useStore.getState().inspector?.sessionId).toBe("lane-1");
  });

  it("stop asks the backend to cancel just that fleet", async () => {
    render(<FleetPanel />);
    started("f1");
    await userEvent.click(screen.getByRole("button", { name: "Stop agents" }));
    expect(ipc.cancelFleet).toHaveBeenCalledWith("s1", "f1");
    // The panel stays until the lanes settle and the backend says so.
    expect(screen.getByText("diff-scan")).toBeInTheDocument();
    // Once every lane has settled there is nothing left to stop.
    act(() => {
      const s = useStore.getState();
      for (const [agent, name] of [
        [0, "diff-scan"],
        [1, "callers"],
      ] as const) {
        s.ingestFleetAgent({
          session: "s1",
          fleet: "f1",
          agent,
          lane: `lane-${agent}`,
          name,
          phase: "done",
          tokens: 10,
          summary: "ok",
        });
      }
    });
    expect(screen.queryByRole("button", { name: "Stop agents" })).not.toBeInTheDocument();
  });
});
