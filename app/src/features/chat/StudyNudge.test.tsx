import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";
import { sampleSession } from "../../test/ipcMock";
import { NUDGE_AFTER_MS, NUDGE_LINGER_MS, resetStudyNudge, StudyNudge } from "./StudyNudge";

/** Render the nudge through a turn long enough to earn the offer. */
function endLongTurn() {
  const view = render(<StudyNudge running />);
  act(() => void vi.advanceTimersByTime(NUDGE_AFTER_MS));
  view.rerender(<StudyNudge running={false} />);
  return view;
}

describe("StudyNudge", () => {
  beforeEach(() => {
    resetAll();
    useStore.setState({ session: sampleSession });
    resetStudyNudge();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("stays quiet through a short turn", () => {
    const { rerender } = render(<StudyNudge running />);
    act(() => void vi.advanceTimersByTime(NUDGE_AFTER_MS - 1));
    expect(screen.queryByRole("status")).toBeNull();
    rerender(<StudyNudge running={false} />);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("says nothing while a long turn is still running", () => {
    render(<StudyNudge running />);
    act(() => void vi.advanceTimersByTime(NUDGE_AFTER_MS * 10));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("offers a ride-along when a long turn ends, then lets it go", () => {
    const { rerender } = render(<StudyNudge running />);
    act(() => void vi.advanceTimersByTime(NUDGE_AFTER_MS));
    rerender(<StudyNudge running={false} />);
    expect(screen.getByRole("status")).toHaveTextContent(/explain what it changed/);
    act(() => void vi.advanceTimersByTime(NUDGE_LINGER_MS));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("opens the work panel on the study cabinet", () => {
    endLongTurn();
    fireEvent.click(screen.getByRole("button", { name: "Study" }));
    expect(useStore.getState().heroGame).toBe("study");
    expect(useStore.getState().rightTab[sampleSession.session_id]).toBe("study");
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("stays silent after being dismissed, and while the game is already on screen", () => {
    const first = endLongTurn();
    fireEvent.click(screen.getByRole("button", { name: /stop offering/i }));
    expect(screen.queryByRole("status")).toBeNull();
    first.unmount();
    endLongTurn();
    expect(screen.queryByRole("status")).toBeNull();

    resetStudyNudge();
    act(() => useStore.setState({ rightTab: { [sampleSession.session_id]: "study" } }));
    endLongTurn();
    expect(screen.queryByRole("status")).toBeNull();
  });
});
