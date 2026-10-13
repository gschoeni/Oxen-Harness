import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { SpeedMeter } from "./SpeedMeter";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { ThroughputEvent } from "../../lib/types";

const reading = (over: Partial<ThroughputEvent>): ThroughputEvent => ({
  session: "s1",
  model: "gemma-4-26b-a4b-q3",
  output_tokens: 0,
  tokens_per_sec: null,
  decode_ms: 0,
  ttft_ms: 820,
  exact: false,
  done: false,
  ...over,
});

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1" },
    threads: { s1: [] },
    runStatus: { s1: "running" },
  });
});

describe("SpeedMeter", () => {
  it("stays out of the way until a call has been timed", () => {
    const { container } = render(<SpeedMeter />);
    act(() => useStore.getState().ingestThroughput(reading({ output_tokens: 1 })));
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the live rate with a sparkline while the reply streams", () => {
    const { container } = render(<SpeedMeter />);
    act(() => {
      const ingest = useStore.getState().ingestThroughput;
      ingest(reading({ output_tokens: 10, decode_ms: 500, tokens_per_sec: 20 }));
      ingest(reading({ output_tokens: 30, decode_ms: 1000, tokens_per_sec: 30 }));
      ingest(reading({ output_tokens: 45, decode_ms: 1500, tokens_per_sec: 30 }));
    });
    expect(screen.getByText("30.0")).toBeInTheDocument();
    expect(container.querySelector(".speed-meter")).toHaveAttribute("data-live", "true");
    // Two measured bars on a fixed-width track; the rest is baseline.
    const spark = container.querySelector(".speed-meter-spark")!;
    expect(spark.querySelectorAll("rect")).toHaveLength(16);
    expect(spark.querySelectorAll("rect:not(.speed-meter-bar-empty)")).toHaveLength(2);
  });

  it("settles on the exact rate and time to first token", () => {
    const { container } = render(<SpeedMeter />);
    act(() => {
      const ingest = useStore.getState().ingestThroughput;
      ingest(reading({ output_tokens: 30, decode_ms: 1000, tokens_per_sec: 30 }));
      ingest(reading({ output_tokens: 446, decode_ms: 10_000, tokens_per_sec: 44.6, exact: true, done: true }));
    });
    expect(screen.getByText("44.6")).toBeInTheDocument();
    expect(screen.getByText("0.8s first token")).toBeInTheDocument();
    const meter = container.querySelector(".speed-meter");
    expect(meter).not.toHaveAttribute("data-live");
    expect(meter?.getAttribute("title")).toContain("446 tokens in 10.0s");
  });

  it("goes quiet when the turn stops mid-call", () => {
    const { container } = render(<SpeedMeter />);
    act(() => useStore.getState().ingestThroughput(reading({ output_tokens: 30, decode_ms: 1000, tokens_per_sec: 30 })));
    act(() => useStore.setState({ runStatus: {} }));
    expect(container.querySelector(".speed-meter")).not.toHaveAttribute("data-live");
    expect(container.querySelector(".speed-meter-spark")).toBeNull();
  });
});
