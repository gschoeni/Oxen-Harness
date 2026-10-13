import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { MemoryMeter } from "./MemoryMeter";
import { MemoryNotices } from "./MemoryNotices";
import { useMemory, useMemoryPolling } from "../../lib/memory";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MemoryNotice, MemoryStatus } from "../../lib/types";

const GB = 2 ** 30;
const status = (over: Partial<MemoryStatus> = {}, pressure: "normal" | "warn" | "critical" = "normal"): MemoryStatus => ({
  system: {
    total_bytes: 36 * GB,
    used_bytes: 30.1 * GB,
    available_bytes: 5.9 * GB,
    compressed_bytes: 11.7 * GB,
    swap_used_bytes: 21 * GB,
    free_percent: 47,
    pressure,
  },
  processes: [{ kind: "chat", model: "gemma-4-26b-a4b-q3", pid: 42, footprint_bytes: 13.2 * GB }],
  notice: null,
  ...over,
});

beforeEach(() => {
  resetAll();
  useMemory.setState({ status: null, banners: [] });
});

function Polled() {
  useMemoryPolling();
  return (
    <>
      <MemoryNotices />
      <MemoryMeter />
    </>
  );
}

describe("MemoryMeter", () => {
  it("stays hidden while no local model runs", () => {
    useMemory.setState({ status: status({ processes: [] }) });
    const { container } = render(<MemoryMeter />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows machine and local-model memory, colored by pressure, with a breakdown", async () => {
    useMemory.setState({ status: status({}, "warn") });
    render(<MemoryMeter />);
    const meter = screen.getByRole("button", { name: /Memory: 30\.1 of 36\.0 GB used, local models 13\.2 GB, pressure High/ });
    expect(meter).toHaveAttribute("data-pressure", "warn");
    expect(meter).toHaveTextContent("30.1/36.0 GB");
    expect(meter).toHaveTextContent("local 13.2 GB");
    await userEvent.click(meter);
    const details = screen.getByRole("dialog", { name: "Memory details" });
    expect(details).toHaveTextContent("High pressure");
    expect(details).toHaveTextContent("gemma-4-26b-a4b-q3");
    expect(details).toHaveTextContent("21.0 GB");
  });

  it("polls the backend and banners a notice when the watchdog stops a model", async () => {
    ipc.memoryStatus.mockResolvedValue(status());
    render(<Polled />);
    expect(await screen.findByRole("button", { name: /^Memory:/ })).toBeInTheDocument();
    const stopped: MemoryNotice = {
      level: "stopped",
      model: "Iris-3B Turbo",
      message: "Stopped Iris-3B Turbo to protect your computer: macOS hit critical memory pressure.",
      at: Math.floor(Date.now() / 1000),
    };
    act(() => ipc.emit("memoryNotice", stopped));
    expect(await screen.findByText("Stopped Iris-3B Turbo")).toBeInTheDocument();
    expect(screen.getByText(/macOS hit critical memory pressure/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("Stopped Iris-3B Turbo")).not.toBeInTheDocument();
  });

  it("lets a warning fade on its own", () => {
    vi.useFakeTimers();
    try {
      useMemory.setState({
        banners: [{ level: "warn", model: "klein", message: "memory is tight", at: 1 }],
      });
      render(<MemoryNotices />);
      expect(screen.getByText("Memory is tight")).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(13_000));
      expect(screen.queryByText("Memory is tight")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
