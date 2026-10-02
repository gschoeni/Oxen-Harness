import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { TokenMeter } from "./TokenMeter";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll, setFeatureFlags } from "../../test/utils";

beforeEach(async () => {
  resetAll();
  await setFeatureFlags({ advanced_settings: true });
});

afterEach(() => setFeatureFlags());

const session = (compression_mode: "off" | "audit" | "on") =>
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", tokens_used: 1200, compression_mode },
  });

describe("TokenMeter compression indicator", () => {
  it("shows the current session's estimated cost", async () => {
    ipc.sessionCost.mockResolvedValueOnce(0.0123);
    session("off");
    useStore.setState({ sessionUsage: { s1: { prompt: 1000, completion: 200 } } });
    render(<TokenMeter />);
    expect(await screen.findByText("$0.01")).toBeInTheDocument();
    expect(ipc.sessionCost).toHaveBeenCalledWith("claude-opus-4-8", 1000, 200);
  });

  it("shows the chat's tree spend, subagents included, once the store has it", async () => {
    ipc.sessionCost.mockClear();
    session("off");
    useStore.setState({
      sessionUsage: { s1: { prompt: 1000, completion: 200 } },
      treeUsage: { s1: { tokens: 250_000, cost: 0.62, unpriced: false } },
    });
    render(<TokenMeter />);
    expect(await screen.findByText("$0.62")).toBeInTheDocument();
    expect(screen.getByText("250,000 tokens used")).toBeInTheDocument();
    expect(ipc.sessionCost).not.toHaveBeenCalled();
  });

  it("re-reads the tree spend as lanes spend, once per burst", async () => {
    vi.useFakeTimers();
    try {
      ipc.sessionTreeUsage.mockResolvedValue({
        rows: [],
        total_cost_usd: 1.5,
        prompt_tokens: 300_000,
        completion_tokens: 5_000,
        has_unpriced_usage: false,
      });
      session("off");
      useStore.setState({ fleets: { f1: { session: "s1", source: "turn", focused: null, lanes: [] } } });
      const budget = { session: "s1", fleet: "f1", tokens: 1, max_tokens: 10, requests: 1, max_requests: 9, spawns: 1, max_spawns: 9 };
      useStore.getState().ingestFleetBudget(budget);
      useStore.getState().ingestFleetBudget({ ...budget, tokens: 2 });
      useStore.getState().ingestFleetBudget({ ...budget, tokens: 3 });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      expect(ipc.sessionTreeUsage).toHaveBeenCalledTimes(1);
      expect(useStore.getState().treeUsage.s1).toEqual({ tokens: 305_000, cost: 1.5, unpriced: false });
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows nothing about compression when the session's mode is off", () => {
    session("off");
    render(<TokenMeter />);
    expect(screen.queryByText(/would save|saved ~/)).not.toBeInTheDocument();
  });

  it("hides the savings readout without the advanced-settings flag", async () => {
    // The composer's picker is the quick way to switch modes; without it the
    // readout is a number the user can't act on from the chat.
    await setFeatureFlags();
    session("audit");
    render(<TokenMeter />);
    expect(screen.queryByText(/would save|saved ~/)).not.toBeInTheDocument();
  });

  it("is armed at ~0 in audit mode before any savings exist", () => {
    // The whole point: audit visibly measures from the first call, so "armed
    // but nothing eligible yet" is distinguishable from "not working".
    session("audit");
    render(<TokenMeter />);
    expect(screen.getByText(/would save ~0/)).toBeInTheDocument();
  });

  it("shows the session's accumulated savings once compression reports them", () => {
    session("audit");
    act(() =>
      useStore.getState().ingestCompression({
        session: "s1",
        mode: "audit",
        saved_tokens: 2100,
        total_saved_tokens: 2100,
        results_compressed: 1,
      }),
    );
    render(<TokenMeter />);
    expect(screen.getByText(/would save ~2\.1k/)).toBeInTheDocument();
  });

  it('reads "saved" (not "would save") when compression is on', () => {
    session("on");
    render(<TokenMeter />);
    expect(screen.getByText(/^saved ~0$/)).toBeInTheDocument();
  });
});
