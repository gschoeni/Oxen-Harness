import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ServiceTierPicker } from "./ServiceTierPicker";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { OxenModelHit } from "../../lib/types";

const sol: OxenModelHit = {
  ...ipc.sampleOxenHits[0],
  id: "gpt-6-1-sol",
  name: "GPT 6.1 Sol",
  pricing: { input_cost_per_token: 2.6e-6, output_cost_per_token: 1.3e-5 },
  service_tiers: [
    { id: "ultrafast", pricing: { input_cost_per_token: 1.56e-5, output_cost_per_token: 7.8e-5 } },
  ],
};

const other = { ...ipc.sampleSession, session_id: "s2", model: "gpt-6-1-sol", service_tiers: ["ultrafast"] };

beforeEach(() => {
  resetAll();
  ipc.searchOxenModels.mockResolvedValue([sol]);
  const session = { ...ipc.sampleSession, session_id: "s1", model: "gpt-6-1-sol", service_tiers: ["ultrafast"] };
  useStore.setState({ session, infos: { s1: session, s2: other } });
});

describe("ServiceTierPicker", () => {
  it("stays out of the toolbar for a model without tiers", () => {
    const session = { ...ipc.sampleSession, session_id: "s1" };
    useStore.setState({ session, infos: { s1: session } });
    const { container } = render(<ServiceTierPicker disabled={false} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("switches this chat to ultrafast, priced, and leaves other chats alone", async () => {
    render(<ServiceTierPicker disabled={false} />);
    await userEvent.click(screen.getByText("Standard"));
    // Each tier carries its own rate, so the cost of going fast is in view.
    expect(await screen.findByLabelText("$15.60 in, $78 out per million tokens")).toBeInTheDocument();
    expect(screen.getByLabelText("$2.60 in, $13 out per million tokens")).toBeInTheDocument();
    await userEvent.click(screen.getByText("Ultrafast"));

    expect(ipc.setServiceTier).toHaveBeenCalledWith("s1", "ultrafast");
    const state = useStore.getState();
    expect(state.session?.service_tier).toBe("ultrafast");
    expect(state.infos.s2.service_tier).toBeUndefined();
    expect(screen.getByText("Ultrafast").closest("button")).toHaveClass("picker-accent");
  });

  it("goes back to the default tier", async () => {
    const session = { ...useStore.getState().session!, service_tier: "ultrafast" };
    useStore.setState({ session, infos: { s1: session } });
    render(<ServiceTierPicker disabled={false} />);
    await userEvent.click(screen.getByText("Ultrafast"));
    await userEvent.click(screen.getByText("Standard"));
    expect(ipc.setServiceTier).toHaveBeenCalledWith("s1", null);
    expect(useStore.getState().session?.service_tier).toBeNull();
  });

  it("says so when the switch fails", async () => {
    ipc.setServiceTier.mockRejectedValueOnce("finish the current turn to switch speed");
    render(<ServiceTierPicker disabled={false} />);
    await userEvent.click(screen.getByText("Standard"));
    await userEvent.click(screen.getByText("Ultrafast"));
    expect(await screen.findByRole("alert")).toHaveTextContent("finish the current turn");
    expect(useStore.getState().session?.service_tier).toBeUndefined();
  });

  it("is disabled mid-turn", () => {
    render(<ServiceTierPicker disabled />);
    expect(screen.getByText("Standard").closest("button")).toBeDisabled();
  });
});
