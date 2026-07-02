import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { SkillsPage } from "./SkillsPage";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { SkillInfo } from "../../lib/types";

const releaseNotes: SkillInfo = {
  name: "release-notes",
  description: "Writes release notes in our house style.",
  instructions: "1. Read the git log.\n2. Group the changes.",
  scope: "global",
  dir: "/home/ox/.oxen-harness/skills/release-notes",
  enabled: true,
};

const reviewChecklist: SkillInfo = {
  name: "review-checklist",
  description: "Runs our code-review checklist.",
  instructions: "Check tests, naming, and docs.",
  scope: "project",
  dir: "/repo/.oxen-harness/skills/review-checklist",
  enabled: false,
};

beforeEach(() => {
  resetAll();
  ipc.listSkills.mockResolvedValue([releaseNotes, reviewChecklist]);
});

describe("SkillsPage", () => {
  it("lists skills with their scope and enabled state", async () => {
    render(<SkillsPage />);
    expect(await screen.findByText("release-notes")).toBeInTheDocument();
    expect(screen.getByText("review-checklist")).toBeInTheDocument();
    expect(screen.getByText("global")).toBeInTheDocument();
    expect(screen.getByText("project")).toBeInTheDocument();
    expect(screen.getByText("review-checklist").closest(".tool-row")).toHaveClass("disabled");
  });

  it("creates a skill through the editor", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));

    await userEvent.type(screen.getByPlaceholderText("release-notes"), "deploy-steps");
    await userEvent.selectOptions(screen.getByLabelText("Skill scope"), "project");
    await userEvent.type(
      screen.getByPlaceholderText(/writes release notes from the git log/i),
      "Walks through our deploy procedure.",
    );
    await userEvent.type(
      screen.getByPlaceholderText(/markdown the agent follows/i),
      "1. Run the deploy script.",
    );
    await userEvent.click(screen.getByRole("button", { name: "Add skill" }));

    await waitFor(() =>
      expect(ipc.saveSkill).toHaveBeenCalledWith(
        "project",
        "deploy-steps",
        "Walks through our deploy procedure.",
        "1. Run the deploy script.",
      ),
    );
    // The editor closes back to the add button.
    expect(screen.getByRole("button", { name: /new skill/i })).toBeInTheDocument();
  });

  it("won't save until name, description, and instructions are filled", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    expect(screen.getByRole("button", { name: "Add skill" })).toBeDisabled();
  });

  it("edits a skill in place without deleting it", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByText("release-notes"));

    const description = screen.getByDisplayValue("Writes release notes in our house style.");
    await userEvent.clear(description);
    await userEvent.type(description, "Better description.");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(ipc.saveSkill).toHaveBeenCalledWith(
        "global",
        "release-notes",
        "Better description.",
        releaseNotes.instructions,
      ),
    );
    expect(ipc.deleteSkill).not.toHaveBeenCalled();
  });

  it("re-homes a skill when its scope changes (save first, then delete)", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByText("release-notes"));
    await userEvent.selectOptions(screen.getByLabelText("Skill scope"), "project");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(ipc.deleteSkill).toHaveBeenCalledWith("global", "release-notes"));
    expect(ipc.saveSkill).toHaveBeenCalledWith(
      "project",
      "release-notes",
      releaseNotes.description,
      releaseNotes.instructions,
    );
  });

  it("shows the backend's message when a save is rejected", async () => {
    ipc.saveSkill.mockRejectedValueOnce("Use a name like `release-notes` — lowercase letters, digits, and hyphens.");
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    await userEvent.type(screen.getByPlaceholderText("release-notes"), "bad");
    await userEvent.type(screen.getByPlaceholderText(/writes release notes from the git log/i), "d");
    await userEvent.type(screen.getByPlaceholderText(/markdown the agent follows/i), "i");
    await userEvent.click(screen.getByRole("button", { name: "Add skill" }));
    expect(await screen.findByText(/use a name like/i)).toBeInTheDocument();
  });

  it("toggles a skill on and off", async () => {
    render(<SkillsPage />);
    await screen.findByText("release-notes");
    await userEvent.click(screen.getByLabelText("Disable release-notes"));
    expect(ipc.setSkillEnabled).toHaveBeenCalledWith("release-notes", false);
  });

  it("deletes a skill only after a second confirming click", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByText("review-checklist"));

    await userEvent.click(screen.getByRole("button", { name: /delete skill/i }));
    expect(ipc.deleteSkill).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: /really delete/i }));
    await waitFor(() =>
      expect(ipc.deleteSkill).toHaveBeenCalledWith("project", "review-checklist"),
    );
  });
});
