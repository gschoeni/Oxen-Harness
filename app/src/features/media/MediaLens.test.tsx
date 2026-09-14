import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { MediaLens } from "./MediaLens";
import { item } from "./ProjectMediaCard.test";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { Project } from "../../lib/types";

const project = (path: string, name: string): Project => ({
  path,
  name,
  description: "",
  instructions: "",
  context: [],
  remote_repo: null,  session_count: 1,
  active: false,
  last_used_at: 0,
});

beforeEach(resetAll);

describe("MediaLens", () => {
  it("loads every project in parallel and lists only those with generations", async () => {
    ipc.listMedia.mockImplementation(async (root: string) =>
      root === "/a"
        ? [item({ id: "a1", prompt: "ox" }), item({ id: "a2", kind: "video", prompt: "race", path: "generations/r.mp4" })]
        : root === "/b"
          ? []
          : Promise.reject(new Error("gone")),
    );
    const enterProject = vi.fn(async () => {});
    useStore.setState({ enterProject });
    render(<MediaLens projects={[project("/a", "Alpha"), project("/b", "Beta"), project("/c", "Gamma")]} />);
    expect(await screen.findByRole("button", { name: "Alpha" })).toBeInTheDocument();
    await waitFor(() => expect(ipc.listMedia).toHaveBeenCalledTimes(3));
    expect(screen.queryByText("Beta")).not.toBeInTheDocument();
    expect(screen.getByText("2 generations · 1 video")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Videos" }));
    expect(screen.queryByTitle("ox")).not.toBeInTheDocument();
    expect(screen.getByTitle("race")).toBeInTheDocument();

    await userEvent.click(screen.getByTitle("race"));
    expect(enterProject).toHaveBeenCalledWith("/a");
    await waitFor(() => expect(useStore.getState().mediaFocus).toBe("a2"));
  });

  it("says so when no project has media", async () => {
    render(<MediaLens projects={[project("/b", "Beta")]} />);
    expect(await screen.findByText(/No generations yet/)).toBeInTheDocument();
  });
});
