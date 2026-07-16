import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ThinkingIndicator } from "./ThinkingIndicator";
import { FRAME_MS, FRAMES_PER_PHRASE } from "../../lib/thinking";
import { useStore } from "../../lib/store";
import { sampleTheme } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  vi.useFakeTimers();
});
afterEach(() => vi.useRealTimers());

describe("ThinkingIndicator", () => {
  it("speaks the theme's thinking phrases and rotates them on the CLI cadence", () => {
    useStore.setState({
      theme: {
        ...sampleTheme,
        voice: { ...sampleTheme.voice, thinking: ["Fording the river", "Yoking the oxen"] },
      },
    });
    render(<ThinkingIndicator />);

    const first = screen.getByText(/Fording the river|Yoking the oxen/).textContent;
    act(() => vi.advanceTimersByTime(FRAME_MS * FRAMES_PER_PHRASE + 5));
    const second = screen.getByText(/Fording the river|Yoking the oxen/).textContent;
    expect(second).not.toBe(first);
    expect(screen.getByText(/\(\d+s\)/)).toBeInTheDocument();
  });

  it("uses write_file verbs while text is streaming (writing mode)", () => {
    useStore.setState({
      theme: {
        ...sampleTheme,
        voice: {
          ...sampleTheme.voice,
          tool_verbs: { ...sampleTheme.voice.tool_verbs, write_file: ["Inscribing the ledger"] },
        },
      },
    });
    render(<ThinkingIndicator writing trailing />);
    expect(screen.getByText(/Inscribing the ledger…/)).toBeInTheDocument();
  });

  it("still animates with a fallback phrase before any theme loads", () => {
    render(<ThinkingIndicator />);
    expect(screen.getByText(/Thinking…/)).toBeInTheDocument();
  });
});
