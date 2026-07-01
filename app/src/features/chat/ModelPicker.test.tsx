import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ModelPicker } from "./ModelPicker";
import { useStore } from "../../lib/store";
import { resetAll } from "../../test/utils";

beforeEach(() => resetAll());

describe("ModelPicker", () => {
  it("shows the active model name when idle", () => {
    useStore.setState({
      session: {
        model: "claude-opus-4-8",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
      },
      cloudModels: [{ id: "claude-opus-4-8", name: "Claude Opus 4.8", builtin: true, selected: true }],
    });
    render(<ModelPicker disabled={false} />);
    expect(screen.getByText("Claude Opus 4.8")).toBeInTheDocument();
  });

  it("shows the local-switch phase + elapsed inline in the switcher", () => {
    useStore.setState({
      localSwitch: { model: "qwen3-1.7b", phase: "loading", startedAt: Date.now() },
    });
    render(<ModelPicker disabled={false} />);
    expect(screen.getByText(/loading model · \d+s/i)).toBeInTheDocument();
  });

  it("explains the one-time first-run wait after a few seconds", () => {
    useStore.setState({
      localSwitch: { model: "qwen3-1.7b", phase: "starting", startedAt: Date.now() - 6000 },
    });
    render(<ModelPicker disabled={false} />);
    expect(screen.getByText(/starting runtime · \d+s/i)).toBeInTheDocument();
    expect(screen.getByText(/first run · one-time/i)).toBeInTheDocument();
  });
});
