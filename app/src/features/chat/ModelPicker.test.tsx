import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { addCloudModel, setModel } from "../../test/ipcMock";
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

  it("reads 'Model' when a mid-turn session reports no model (never a blank button)", () => {
    useStore.setState({
      session: {
        model: "",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 0,
        compression_mode: "off",
      },
      cloudModels: [],
    });
    render(<ModelPicker disabled={false} />);
    expect(screen.getByText("Model")).toBeInTheDocument();
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
    // …the id is a tooltip only, not a second line of text.
    expect(screen.queryByText("claude-sonnet-4-6")).toBeNull();
    expect(screen.getByTitle("claude-sonnet-4-6")).toHaveClass("menu-item");
    // …and the installed local model is labeled free.
    expect(await screen.findByText("Qwen3 8B · Q4_K_M")).toBeInTheDocument();
    expect(screen.getByText("free")).toBeInTheDocument();
  });

  it("flags a saved model the endpoint's catalog no longer lists", async () => {
    useStore.setState({
      session: {
        model: "muse-spark-1-1",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
        compression_mode: "off",
      },
      cloudModels: [
        { id: "muse-spark-1-1", name: "Muse Spark 1.1", selected: true },
        { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", selected: false },
      ],
      localSwitch: null,
    });
    render(<ModelPicker disabled={false} />);
    fireEvent.click(screen.getByText("Muse Spark 1.1"));
    // Once the catalog has answered, a listed model shows its rate and an
    // unlisted one says so instead of silently showing nothing.
    await screen.findByLabelText("$3 in, $15 out per million tokens");
    expect(screen.getByText("not in catalog")).toHaveClass("menu-rate-note");
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

  it("lists the active model first, then the rest alphabetically", () => {
    useStore.setState({
      session: {
        model: "zed",
        workspace: "/x",
        session_id: "s1",
        tokens_used: 0,
        context_tokens: 0,
        context_window: 200000,
        compression_mode: "off",
      },
      cloudModels: [
        { id: "mid", name: "mango 10", selected: false },
        { id: "zed", name: "Zed", selected: true },
        { id: "mid2", name: "Mango 2", selected: false },
        { id: "apple", name: "apple", selected: false },
      ],
      localSwitch: null,
    });
    render(<ModelPicker disabled={false} />);
    fireEvent.click(screen.getByText("Zed"));
    const names = screen.getAllByRole("option").map((o) => o.textContent);
    expect(names.slice(0, 4).map((n) => n?.replace(/\$.*/, "").trim())).toEqual([
      "Zed",
      "apple",
      "Mango 2",
      "mango 10",
    ]);
  });

  describe("type-to-find", () => {
    const setup = () => {
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
      return screen.getByLabelText("Search models");
    };

    it("filters saved models as you type", async () => {
      const input = setup();
      await screen.findByLabelText("$3 in, $15 out per million tokens");
      fireEvent.change(input, { target: { value: "zzz" } });
      expect(screen.queryByTitle("claude-sonnet-4-6")).toBeNull();
      expect(screen.getByText(/No models match/)).toBeInTheDocument();
    });

    it("offers an unsaved catalog model and adds + selects it on click", async () => {
      const input = setup();
      await screen.findByLabelText("$3 in, $15 out per million tokens");
      fireEvent.change(input, { target: { value: "muse" } });
      fireEvent.click(await screen.findByTitle("Add muse-spark-1-1 to your models"));
      await waitFor(() => expect(addCloudModel).toHaveBeenCalledWith("muse-spark-1-1", "Muse Spark 1.1"));
      await waitFor(() => expect(setModel).toHaveBeenCalledWith("muse-spark-1-1"));
    });

    it("never offers non-chat models", async () => {
      const input = setup();
      await screen.findByLabelText("$3 in, $15 out per million tokens");
      fireEvent.change(input, { target: { value: "pix" } });
      expect(screen.queryByText("Pix Gen")).toBeNull();
    });

    it("reports a failed add and keeps the menu open", async () => {
      addCloudModel.mockRejectedValueOnce("disk full");
      const input = setup();
      await screen.findByLabelText("$3 in, $15 out per million tokens");
      fireEvent.change(input, { target: { value: "muse" } });
      fireEvent.click(await screen.findByTitle("Add muse-spark-1-1 to your models"));
      expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't add muse-spark-1-1: disk full");
      expect(setModel).not.toHaveBeenCalled();
    });
  });
});
