import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Hero } from "./Hero";
import { useStore } from "../../lib/store";
import { sampleTheme, studyBatch, studyFlag, studyProfile } from "../../test/ipcMock";
import { STUDY_NEEDS_KEY, studyHost } from "./games/studyHost";
import { STUDY_BATCH, STUDY_PROFILE } from "./games/study";
import { resetAll } from "../../test/utils";

beforeEach(() => resetAll());

describe("Hero usage rows", () => {
  it("replaces the next-landmark row with total dollars spent", () => {
    useStore.setState({
      theme: {
        ...sampleTheme,
        voice: {
          ...sampleTheme.voice,
          flavor_bottom: [
            ["Next landmark", "128000 tokens"],
            ["Total tokens used", "0 tokens"],
          ],
        },
      },
      totalTokensUsed: 5715185,
      totalCostUsd: 12.345,
    });

    render(<Hero examples={[]} busy={false} onPick={() => {}} />);

    expect(screen.queryByText("Next landmark")).not.toBeInTheDocument();
    expect(screen.getByText(/Total dollars spent/)).toBeInTheDocument();
    expect(screen.getByText("$12.35")).toBeInTheDocument();
  });

  it("shows total spend when a custom theme has no spend slot", () => {
    useStore.setState({ theme: sampleTheme, totalCostUsd: 0.0042 });

    render(<Hero examples={[]} busy={false} onPick={() => {}} />);

    expect(screen.getByText(/Total dollars spent/)).toBeInTheDocument();
    expect(screen.getByText("$0.0042")).toBeInTheDocument();
  });
});

describe("Hero default game", () => {
  const show = () => render(<Hero examples={[]} busy={false} onPick={() => {}} />);
  const cabinet = () => document.querySelector(".hero-game")?.getAttribute("data-game");

  it("opens on the study cabinet for someone who has never picked one", () => {
    useStore.setState({ theme: sampleTheme });
    show();
    expect(cabinet()).toBe("study");
    expect(screen.getByRole("tab", { name: "Study" })).toHaveAttribute("aria-selected", "true");
  });

  it("lets a theme's game, and then a saved choice, win over the default", () => {
    const themed = { ...sampleTheme, style: { ...sampleTheme.style, game: "hunt" } };
    useStore.setState({ theme: themed });
    const { unmount } = show();
    expect(cabinet()).toBe("hunt");
    unmount();

    useStore.setState({ theme: themed, heroGame: "tumbleweed" });
    show();
    expect(cabinet()).toBe("tumbleweed");
  });

  it("idles on the title card without a chat, a profile, or a model call", async () => {
    studyProfile.mockRejectedValue(new Error("no profile yet"));
    useStore.setState({ theme: sampleTheme, session: null });
    show();
    // The attract screen stands on its own: no error card, nothing in flight.
    expect(await screen.findByText("PLAY TO MAP WHAT")).toBeInTheDocument();
    expect(screen.queryByText("TRAIL BLOCKED")).not.toBeInTheDocument();
    expect(studyBatch).not.toHaveBeenCalled();
  });
});

describe("the study cabinet without an API key", () => {
  const request = { mode: "expedition" as const, count: 5, exclude: [] };

  it("says a key is needed instead of relaying the provider's 401", async () => {
    studyBatch.mockRejectedValue("Oxen API error (401): unauthorized");
    await expect(studyHost("s1", true).perform(STUDY_BATCH, request)).rejects.toThrow(STUDY_NEEDS_KEY);
    // The saved profile needs no model, so the title card still fills in.
    await expect(studyHost("s1", true).perform(STUDY_PROFILE, undefined)).resolves.toBeTruthy();
  });

  it("leaves other failures, and endpoints that take no key, alone", async () => {
    await expect(studyHost("s1", true).perform(STUDY_BATCH, request)).resolves.toBeTruthy();
    studyBatch.mockRejectedValue("Oxen API error (429): slow down");
    await expect(studyHost("s1", false).perform(STUDY_BATCH, request)).rejects.toBe("Oxen API error (429): slow down");
  });

  it("opens a question's file in the editor pane and retires a flagged one", async () => {
    const opened: string[] = [];
    const host = studyHost("s1", false, (path) => opened.push(path));
    await host.perform("study.open", { path: "crates/agent/src/turn.rs" });
    expect(opened).toEqual(["crates/agent/src/turn.rs"]);
    await host.perform("study.flag", { question_id: "q1" });
    expect(studyFlag).toHaveBeenCalledWith("s1", "q1");
  });
});
