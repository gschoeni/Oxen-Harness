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
        banners: [{ level: "warn", model: "klein", message: "memory is tight: klein wants about 3.0 GB and 2.1 GB is free.", at: 1 }],
      });
      render(<MemoryNotices />);
      expect(screen.getByText("Memory is tight")).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(13_000));
      expect(screen.queryByText("Memory is tight")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers to quit the apps holding the most memory, after a confirm", async () => {
    const message =
      "Stopped gemma to protect your computer: macOS hit critical memory pressure (32.1 of 36.0 GB used). " +
      "Using the most: Docker (4.0 GB) and claude (2.4 GB).";
    useMemory.setState({
      banners: [
        {
          level: "stopped",
          model: "gemma",
          message,
          at: 1,
          hogs: [
            { name: "Docker", bytes: 4 * GB, bundle: true },
            { name: "claude", bytes: 2.4 * GB, bundle: false },
          ],
        },
      ],
    });
    ipc.quitMemoryHog.mockResolvedValue(4 * GB);
    render(<MemoryNotices />);

    // The body says why, once — not the title again, not the list the
    // buttons already show.
    expect(screen.getByText("macOS hit critical memory pressure (32.1 of 36.0 GB used).")).toBeInTheDocument();
    // A bare process is stopped, an app quit.
    expect(screen.getByRole("button", { name: /^Stop claude/ })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /^Quit Docker/ }));
    expect(ipc.quitMemoryHog).not.toHaveBeenCalled();
    expect(screen.getByText(/It can still ask you to save first/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Quit Docker" }));

    expect(ipc.quitMemoryHog).toHaveBeenCalledWith("Docker");
    expect(await screen.findByText("Quit Docker — about 4.0 GB freed.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Quit Docker/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Stop claude/ })).toBeInTheDocument();
  });

  it("shows why an app didn't quit", async () => {
    useMemory.setState({
      banners: [
        { level: "refused", model: "gemma", message: "no room", at: 1, hogs: [{ name: "Keynote", bytes: GB, bundle: true }] },
      ],
    });
    ipc.quitMemoryHog.mockRejectedValue("Keynote didn't quit: it may be waiting for you to save something");
    render(<MemoryNotices />);
    await userEvent.click(screen.getByRole("button", { name: /^Quit Keynote/ }));
    await userEvent.click(screen.getByRole("button", { name: "Quit Keynote" }));
    expect(await screen.findByText(/waiting for you to save/)).toBeInTheDocument();
  });
});
