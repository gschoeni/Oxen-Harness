import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { Hero } from "./Hero";
import { useStore } from "../../lib/store";
import { sampleTheme } from "../../test/ipcMock";
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
});
