import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { asRepo, MediaDestinationPicker } from "./MediaDestinationPicker";
import { dispatchSlashCommand } from "../chat/slashDispatch";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({
    session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" },
  });
});

async function openPicker() {
  const user = userEvent.setup();
  render(<MediaDestinationPicker root="/w" />);
  await user.click(await screen.findByRole("button", { name: "Hub repository: me/playground" }));
  // The namespaces arrive, the current one selected.
  expect(await screen.findByRole("button", { name: /^me/, pressed: true })).toBeInTheDocument();
  return user;
}

describe("MediaDestinationPicker", () => {
  it("shows where generations go and switches to a listed repository", async () => {
    const user = await openPicker();
    expect(ipc.mediaDestination).toHaveBeenCalledWith("/w");
    expect(screen.getByRole("option", { name: /playground/ })).toHaveAttribute("aria-selected", "true");

    await user.click(screen.getByRole("button", { name: /^Studio/ }));
    await user.click(screen.getByRole("option", { name: /shots/ }));
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "Studio/shots");
    // Closed, and the trigger names the new home.
    expect(await screen.findByRole("button", { name: "Hub repository: Studio/shots" })).toBeInTheDocument();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("offers a new name only where the hub will create it", async () => {
    const user = await openPicker();
    const filter = screen.getByRole("searchbox", { name: "Filter repositories" });
    await user.type(filter, "moodboard");
    expect(screen.getByRole("option", { name: /Create me\/moodboard/ })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /^ox/ }));
    expect(screen.queryByRole("option", { name: /Create/ })).not.toBeInTheDocument();
    expect(screen.getByText("No matching repositories.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /^me/ }));
    await user.click(filter);
    await user.keyboard("{Enter}");
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "me/moodboard");
  });

  it("keeps the menu open with the hub's reason when a choice is refused", async () => {
    ipc.setMediaDestination.mockRejectedValueOnce(
      "you can't write to `ox/public` (your access: viewer); it needs the admin or editor role",
    );
    const user = await openPicker();
    await user.type(screen.getByRole("searchbox", { name: "Filter repositories" }), "ox/public");
    await user.click(screen.getByRole("option", { name: /Use ox\/public/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("your access: viewer");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hub repository: me/playground" })).toBeInTheDocument();
  });

  it("a project's own choice can be dropped back to the default", async () => {
    ipc.mediaDestination.mockResolvedValueOnce({
      repo: "Studio/shots",
      source: "project",
      web_url: "https://hub.oxen.ai/Studio/shots",
    });
    const user = userEvent.setup();
    render(<MediaDestinationPicker root="/w" />);
    await user.click(await screen.findByRole("button", { name: "Hub repository: Studio/shots" }));
    await user.click(await screen.findByRole("option", { name: /Use the default/ }));
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", null);
  });

  it("opens when /media-repo asks", async () => {
    render(<MediaDestinationPicker root="/w" />);
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    await act(() => dispatchSlashCommand("/media-repo"));
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
    expect(useStore.getState().mediaDestinationPick).toBe(false);
  });
});

describe("/media-repo", () => {
  it("checks and switches with an argument, and reports a refusal", async () => {
    await dispatchSlashCommand("/media-repo Studio/shots");
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "Studio/shots");
    expect(useStore.getState().mediaDestinations["/w"]?.repo).toBe("Studio/shots");

    ipc.setMediaDestination.mockRejectedValueOnce("`nope/x` doesn't exist, and new repositories can only go in `me`");
    await dispatchSlashCommand("/hub-repo nope/x");
    expect(useStore.getState().mediaDestinations["/w"]?.repo).toBe("Studio/shots");

    await dispatchSlashCommand("/media-repo clear");
    expect(ipc.setMediaDestination).toHaveBeenLastCalledWith("/w", null);
  });
});

describe("asRepo", () => {
  it("accepts namespace/name only", () => {
    expect(asRepo(" /ox/art-1.v2/ ")).toBe("ox/art-1.v2");
    for (const bad of ["ox", "ox/", "ox/a/b", "ox/a b", ""]) expect(asRepo(bad)).toBeNull();
  });
});
