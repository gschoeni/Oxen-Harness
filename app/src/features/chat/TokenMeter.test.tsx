import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { TokenMeter } from "./TokenMeter";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
});

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

  it("shows nothing about compression when the session's mode is off", () => {
    session("off");
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
