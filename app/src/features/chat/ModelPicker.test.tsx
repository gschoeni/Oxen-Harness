import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

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
      compression_mode: "off",
      },
      cloudModels: [{ id: "claude-opus-4-8", name: "Claude Opus 4.8", selected: true }],
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

  it("offers both local setup and cloud-model configuration in the menu", () => {
    useStore.setState({
      session: {
        model: "claude-opus-4-8",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
        compression_mode: "off",
      },
      cloudModels: [{ id: "claude-opus-4-8", name: "Claude Opus 4.8", selected: true }],
      localSwitch: null,
    });
    render(<ModelPicker disabled={false} />);
    fireEvent.click(screen.getByText("Claude Opus 4.8"));
    expect(screen.getByText("Set up a local model…")).toBeInTheDocument();
    expect(screen.getByText("Configure a cloud model…")).toBeInTheDocument();
  });

  it("shows per-million rates on cloud rows and 'free' on local rows", async () => {
    useStore.setState({
      session: {
        model: "claude-sonnet-4-6",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
        compression_mode: "off",
      },
      cloudModels: [{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", selected: true }],
      localSwitch: null,
    });
    render(<ModelPicker disabled={false} />);
    fireEvent.click(screen.getByText("Claude Sonnet 4.6"));
    // The cloud row carries its catalog rate (from search_oxen_models) as an
    // in/out pair of figures, with the unit said once on the section head…
    const rate = await screen.findByLabelText("$3 in, $15 out per million tokens");
    expect([...rate.querySelectorAll("b")].map((b) => b.textContent)).toEqual(["$3", "$15"]);
    expect(screen.getByText("$ per 1M tokens")).toHaveClass("menu-head-aside");
    // …and its id sits under the display name rather than in the hint column.
    expect(screen.getByText("claude-sonnet-4-6")).toHaveClass("menu-id");
    // …and the installed local model is labeled free.
    expect(await screen.findByText("Qwen3 8B · Q4_K_M")).toBeInTheDocument();
    expect(screen.getByText("free")).toBeInTheDocument();
  });

  it("omits the id line when the display name already is the id", () => {
    useStore.setState({
      session: {
        model: "claude-sonnet-4-6",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
        compression_mode: "off",
      },
      cloudModels: [{ id: "claude-sonnet-4-6", name: "claude-sonnet-4-6", selected: true }],
      localSwitch: null,
    });
    render(<ModelPicker disabled={false} />);
    fireEvent.click(screen.getByText("claude-sonnet-4-6"));
    expect(screen.getAllByText("claude-sonnet-4-6").filter((el) => el.closest(".menu-item"))).toHaveLength(1);
    expect(document.querySelector(".menu-id")).toBeNull();
  });

  it("jumps to the cloud-models settings page from the configure button", () => {
    useStore.setState({
      session: {
        model: "claude-opus-4-8",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
        compression_mode: "off",
      },
      cloudModels: [{ id: "claude-opus-4-8", name: "Claude Opus 4.8", selected: true }],
      localSwitch: null,
    });
    render(<ModelPicker disabled={false} />);
    fireEvent.click(screen.getByText("Claude Opus 4.8"));
    fireEvent.click(screen.getByText("Configure a cloud model…"));
    const s = useStore.getState();
    expect(s.settingsOpen).toBe(true);
    expect(s.settingsPage).toBe("cloud-models");
  });
});
