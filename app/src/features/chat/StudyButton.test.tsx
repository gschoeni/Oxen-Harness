import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";
import { sampleSession } from "../../test/ipcMock";
import { StudyButton } from "./StudyButton";

describe("StudyButton", () => {
  beforeEach(() => {
    resetAll();
    useStore.setState({ session: sampleSession });
  });

  it("is there whether or not the agent is working", () => {
    const { rerender } = render(<StudyButton busy={false} />);
    expect(screen.getByRole("button", { name: "Study" })).toBeInTheDocument();
    rerender(<StudyButton busy />);
    expect(screen.getByRole("button", { name: "Study while it works" })).toBeInTheDocument();
  });

  it("opens the work panel on the study cabinet", () => {
    render(<StudyButton busy={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Study" }));
    expect(useStore.getState().heroGame).toBe("study");
    expect(useStore.getState().rightTab[sampleSession.session_id]).toBe("study");
  });

  it("steps aside while the game is already on screen", () => {
    render(<StudyButton busy={false} />);
    act(() => useStore.setState({ rightTab: { [sampleSession.session_id]: "study" } }));
    expect(screen.queryByRole("button")).toBeNull();
  });
});
