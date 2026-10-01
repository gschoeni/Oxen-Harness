import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Hero } from "./Hero";
import { useStore } from "../../lib/store";
import { sampleTheme, studyBatch, studyProfile } from "../../test/ipcMock";
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
