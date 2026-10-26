import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
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
  await user.click(await screen.findByRole("button", { name: "Repository: me/playground" }));
  // The namespaces arrive, the current one selected.
  expect(await screen.findByRole("combobox", { name: "Namespace" })).toHaveValue("me");
  return user;
}

const repos = () => within(screen.getByRole("listbox", { name: "Repositories" }));

describe("MediaDestinationPicker", () => {
  it("lists the current namespace's repositories and switches to one", async () => {
    const user = await openPicker();
    expect(repos().getByRole("option", { name: /playground/ })).toHaveAttribute("aria-selected", "true");

    await user.selectOptions(screen.getByRole("combobox", { name: "Namespace" }), "Studio");
    await user.click(repos().getByRole("option", { name: /shots/ }));
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "Studio/shots");
    expect(await screen.findByRole("button", { name: "Repository: Studio/shots" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("searches every namespace at once, and Enter takes the highlighted row", async () => {
    const user = await openPicker();
    await user.type(screen.getByRole("searchbox", { name: "Search repositories" }), "play");
    const names = repos()
      .getAllByRole("option")
      .map((o) => o.textContent);
    // Matches from every namespace, then the typed name as a new repository.
    expect(names).toEqual(["me/playground in use", "ox/playground", " Create me/playcreated on first use"]);
    await user.keyboard("{ArrowDown}{Enter}");
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "ox/playground");
  });

  it("offers a new name only where the hub will create it", async () => {
    const user = await openPicker();
    const namespace = screen.getByRole("combobox", { name: "Namespace" });
    await user.selectOptions(namespace, "ox");
    await user.type(screen.getByRole("searchbox", { name: "Search repositories" }), "moodboard");
    expect(repos().queryByRole("option", { name: /Create/ })).not.toBeInTheDocument();

    await user.selectOptions(namespace, "me");
    await user.type(screen.getByRole("searchbox", { name: "Search repositories" }), "moodboard");
    await user.click(repos().getByRole("option", { name: /Create me\/moodboard/ }));
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "me/moodboard");
  });

  it("keeps the picker open with the hub's reason when a choice is refused", async () => {
    ipc.setMediaDestination.mockRejectedValueOnce(
      "you can't write to `ox/public` (your access: viewer); it needs the admin or editor role",
    );
    const user = await openPicker();
    await user.type(screen.getByRole("searchbox", { name: "Search repositories" }), "ox/public");
    await user.click(repos().getByRole("option", { name: /Use ox\/public/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("your access: viewer");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Repository: me/playground" })).toBeInTheDocument();
  });

  it("lists the hub once per run, and refresh asks again", async () => {
    const user = await openPicker();
    await user.click(screen.getByRole("button", { name: "Repository: me/playground" }));
    await user.click(screen.getByRole("button", { name: "Repository: me/playground" }));
    await screen.findByRole("combobox", { name: "Namespace" });
    expect(ipc.listMediaNamespaces).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Refresh the list from Oxen.ai" }));
    expect(ipc.listMediaNamespaces).toHaveBeenCalledTimes(2);
  });

  it("a project's own choice can be dropped back to the default", async () => {
    ipc.mediaDestination.mockResolvedValueOnce({
      repo: "Studio/shots",
      source: "project",
      web_url: "https://hub.oxen.ai/Studio/shots",
    });
    const user = userEvent.setup();
    render(<MediaDestinationPicker root="/w" />);
    await user.click(await screen.findByRole("button", { name: "Repository: Studio/shots" }));
    await user.click(screen.getByRole("button", { name: /Use the default/ }));
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", null);
  });
});

describe("/repository", () => {
  it("checks and switches with an argument, and reports a refusal", async () => {
    await dispatchSlashCommand("/repository Studio/shots");
    expect(ipc.setMediaDestination).toHaveBeenCalledWith("/w", "Studio/shots");
    expect(useStore.getState().mediaDestinations["/w"]?.repo).toBe("Studio/shots");

    ipc.setMediaDestination.mockRejectedValueOnce("`nope/x` doesn't exist, and new repositories can only go in `me`");
    await dispatchSlashCommand("/repo nope/x");
    expect(useStore.getState().mediaDestinations["/w"]?.repo).toBe("Studio/shots");

    await dispatchSlashCommand("/repository clear");
    expect(ipc.setMediaDestination).toHaveBeenLastCalledWith("/w", null);
  });

  it("opens Settings → Connection without an argument", async () => {
    await dispatchSlashCommand("/repository");
    expect(useStore.getState().settingsOpen).toBe(true);
    expect(useStore.getState().settingsPage).toBe("connection");
    expect(ipc.setMediaDestination).not.toHaveBeenCalled();
  });
});

describe("asRepo", () => {
  it("accepts namespace/name only", () => {
    expect(asRepo(" /ox/art-1.v2/ ")).toBe("ox/art-1.v2");
    for (const bad of ["ox", "ox/", "ox/a/b", "ox/a b", ""]) expect(asRepo(bad)).toBeNull();
  });
});
