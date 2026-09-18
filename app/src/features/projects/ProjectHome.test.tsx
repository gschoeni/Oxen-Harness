import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ProjectHome } from "./ProjectHome";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";
import type { Project } from "../../lib/types";

function project(overrides: Partial<Project> = {}): Project {
  return {
    path: "/work/app",
    name: "app",
    description: "",
    instructions: "",
    context: [],
    remote_repo: null,
    session_count: 0,
    active: true,
    last_used_at: null,
    ...overrides,
  };
}

beforeEach(() => {
  resetAll();
});

it.each([{ isComposing: true }, { keyCode: 229 }])(
  "does not submit a project prompt while native composition is committing (%j)",
  (nativeEvent) => {
    render(<ProjectHome project={project()} onBack={() => {}} onProjectChanged={() => {}} />);
    const box = screen.getByRole("textbox", { name: "Ask about this project" });
    const submit = vi.spyOn(HTMLFormElement.prototype, "requestSubmit").mockImplementation(() => {});
    try {
      fireEvent.change(box, { target: { value: "日本語" } });
      expect(fireEvent.keyDown(box, { key: "Enter", ...nativeEvent })).toBe(true);
      expect(submit).not.toHaveBeenCalled();
      expect(box).toHaveValue("日本語");
      fireEvent.keyDown(box, { key: "Enter" });
      expect(submit).toHaveBeenCalledOnce();
    } finally {
      submit.mockRestore();
    }
  },
);

describe("the project's repository card", () => {
  it("saves a well-formed namespace/name and keeps the other fields", async () => {
    const onProjectChanged = vi.fn(async () => {});
    render(<ProjectHome project={project({ description: "ship it" })} onBack={() => {}} onProjectChanged={onProjectChanged} />);

    const field = screen.getByLabelText("Remote Oxen repository");
    expect(screen.getByText(/None yet/)).toBeTruthy();
    await userEvent.type(field, "/ox/my-app/{Enter}");

    await waitFor(() =>
      expect(ipc.updateProject).toHaveBeenCalledWith("/work/app", "app", "ship it", "", "ox/my-app"),
    );
    expect(onProjectChanged).toHaveBeenCalledWith(expect.objectContaining({ remote_repo: "ox/my-app" }));
  });

  it("refuses a malformed remote before it reaches the backend", async () => {
    render(<ProjectHome project={project()} onBack={() => {}} onProjectChanged={async () => {}} />);

    await userEvent.type(screen.getByLabelText("Remote Oxen repository"), "my-app{Enter}");
    expect(screen.getByText(/Use the form/)).toBeTruthy();
    expect(ipc.updateProject).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Save repository" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("links a set repository to the connected hub, and an empty field clears it", async () => {
    render(<ProjectHome project={project({ remote_repo: "ox/art" })} onBack={() => {}} onProjectChanged={async () => {}} />);

    const link = await screen.findByRole("button", { name: "Open repository on the hub" });
    await userEvent.click(link);
    expect(ipc.openExternal).toHaveBeenCalledWith(expect.stringMatching(/^https:\/\/.+\/ox\/art$/));

    await userEvent.clear(screen.getByLabelText("Remote Oxen repository"));
    await userEvent.click(screen.getByRole("button", { name: "Save repository" }));
    await waitFor(() => expect(ipc.updateProject).toHaveBeenCalledWith("/work/app", "app", "", "", null));
  });

  it("threads the remote through the other project edits untouched", async () => {
    render(<ProjectHome project={project({ remote_repo: "ox/art" })} onBack={() => {}} onProjectChanged={async () => {}} />);

    const name = screen.getByLabelText("Project name");
    await userEvent.clear(name);
    await userEvent.type(name, "renamed{Enter}");
    await waitFor(() => expect(ipc.updateProject).toHaveBeenCalledWith("/work/app", "renamed", "", "", "ox/art"));
  });
});
