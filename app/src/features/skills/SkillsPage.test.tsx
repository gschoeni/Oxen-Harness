import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { SkillsPage } from "./SkillsPage";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { SkillInfo } from "../../lib/types";

const releaseNotes: SkillInfo = {
  name: "release-notes",
  description: "Writes release notes in our house style.",
  instructions: "# Steps\n\n1. Read the **git log** with `read_file`.\n2. Group the changes.",
  scope: "global",
  dir: "/home/ox/.oxen-harness/skills/release-notes",
  enabled: true,
};

/** A minimal ToolInfo for the autocomplete vocabulary. */
const tool = (name: string) => ({
  name,
  description: "",
  default_description: "",
  parameters: {},
  enabled: true,
  builtin: true,
  config: {},
});

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
  // An active project so scope labels can name it.
  useStore.setState({
    session: { ...ipc.sampleSession, workspace: "/repo" },
    projects: [{ path: "/repo", name: "OxenHarness", session_count: 1, active: true }],
  });
  ipc.listSkills.mockResolvedValue([releaseNotes, reviewChecklist]);
  ipc.listTools.mockResolvedValue([tool("read_file"), tool("run_shell")]);
});

/** Open a skill's reading view from the list. */
async function openSkill(name: string) {
  await userEvent.click(await screen.findByRole("button", { name: `Open skill ${name}` }));
}

describe("SkillsPage", () => {
  it("explains the tools vs skills split and cross-links to Tools", async () => {
    render(<SkillsPage />);
    await screen.findByText("release-notes");
    expect(screen.getByText(/knows how to do/i)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Tools" }));
    expect(useStore.getState().settingsPage).toBe("tools");
  });

  it("groups skills by context — this project (named) first, then global", async () => {
    render(<SkillsPage />);
    expect(await screen.findByText("release-notes")).toBeInTheDocument();
    expect(screen.getByText("review-checklist")).toBeInTheDocument();
    expect(screen.getByText("This project · OxenHarness")).toBeInTheDocument();
    expect(screen.getByText("Global · every project")).toBeInTheDocument();
    expect(screen.getByText("review-checklist").closest(".tool-row")).toHaveClass("disabled");

    // The project group renders above the global one.
    const labels = [...document.querySelectorAll(".skills-group-label")].map(
      (el) => el.textContent,
    );
    expect(labels[0]).toContain("This project");
  });

  it("opens a skill's reading view with rendered markdown, and navigates back", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");

    // The instructions render as markdown, not raw text.
    expect(screen.getByRole("heading", { name: "Steps" })).toBeInTheDocument();
    expect(screen.getByText("git log")).toBeInTheDocument();
    expect(screen.queryByText(/# Steps/)).toBeNull();
    // Identity + storage location are shown.
    expect(screen.getByText("Writes release notes in our house style.")).toBeInTheDocument();
    expect(screen.getByText(/skills\/release-notes\/SKILL\.md/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /all skills/i }));
    expect(screen.getByRole("button", { name: /new skill/i })).toBeInTheDocument();
  });

  it("toggles a skill from the list and from the reading view", async () => {
    render(<SkillsPage />);
    await screen.findByText("release-notes");
    await userEvent.click(screen.getByLabelText("Disable release-notes"));
    expect(ipc.setSkillEnabled).toHaveBeenCalledWith("release-notes", false);

    await openSkill("review-checklist");
    await userEvent.click(screen.getByLabelText("Enable review-checklist"));
    expect(ipc.setSkillEnabled).toHaveBeenCalledWith("review-checklist", true);
  });

  it("creates a skill through the editor and lands on its reading view", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));

    await userEvent.type(screen.getByPlaceholderText("release-notes"), "deploy-steps");
    await userEvent.selectOptions(screen.getByLabelText("Skill scope"), "project");
    await userEvent.type(
      screen.getByPlaceholderText(/writes release notes from the git log/i),
      "Walks through our deploy procedure.",
    );
    await userEvent.type(screen.getByLabelText("Instructions markdown"), "1. Run the deploy script.");

    // The new skill will be in the reloaded list, so the show view can render it.
    const deploySteps: SkillInfo = {
      name: "deploy-steps",
      description: "Walks through our deploy procedure.",
      instructions: "1. Run the deploy script.",
      scope: "project",
      dir: "/repo/.oxen-harness/skills/deploy-steps",
      enabled: true,
    };
    ipc.listSkills.mockResolvedValue([deploySteps, releaseNotes, reviewChecklist]);

    await userEvent.click(screen.getByRole("button", { name: "Add skill" }));

    await waitFor(() =>
      expect(ipc.saveSkill).toHaveBeenCalledWith(
        "project",
        "deploy-steps",
        "Walks through our deploy procedure.",
        "1. Run the deploy script.",
      ),
    );
    // Landed on the reading view of the new skill.
    expect(await screen.findByText("Walks through our deploy procedure.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /all skills/i })).toBeInTheDocument();
  });

  it("previews the instructions as markdown while editing", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));

    // Write mode shows the raw markdown in a textarea.
    const editor = screen.getByLabelText<HTMLTextAreaElement>("Instructions markdown");
    expect(editor.value).toContain("# Steps");

    await userEvent.click(screen.getByRole("button", { name: "Preview" }));
    expect(screen.getByRole("heading", { name: "Steps" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Instructions markdown")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Write" }));
    expect(screen.getByLabelText("Instructions markdown")).toBeInTheDocument();
  });

  it("highlights known tool references in the reading view and preview", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");

    // `read_file` matches a registered tool, so it renders as a reference chip.
    const ref = document.querySelector("code.tool-ref");
    expect(ref?.textContent).toBe("read_file");
  });

  it("autocompletes tool names after a backtick in the editor", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));

    const editor = screen.getByLabelText<HTMLTextAreaElement>("Instructions markdown");
    await userEvent.type(editor, "\nThen run `run");

    // The suggestion list offers the matching tool; accepting completes the
    // reference with its closing backtick.
    await userEvent.click(screen.getByRole("option", { name: "run_shell" }));
    expect(editor.value).toContain("Then run `run_shell`");

    // The reference summary now lists it as a known tool.
    expect(screen.getByLabelText("Referenced tools")).toHaveTextContent("run_shell");
  });

  it("warns about backticked names that don't match any tool", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));

    const editor = screen.getByLabelText<HTMLTextAreaElement>("Instructions markdown");
    await userEvent.type(editor, "\nUse `run_shel` here.");

    const warning = document.querySelector(".skill-ref-chip.unknown");
    expect(warning?.textContent).toContain("run_shel");
  });

  it("won't save until name, description, and instructions are filled", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    expect(screen.getByRole("button", { name: "Add skill" })).toBeDisabled();
  });

  it("names the active project in the scope choice", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    expect(
      screen.getByRole("option", { name: "This project only — OxenHarness" }),
    ).toBeInTheDocument();

    // Choosing it makes the hint say exactly where the skill will land.
    await userEvent.selectOptions(screen.getByLabelText("Skill scope"), "project");
    expect(screen.getByText(/saved into OxenHarness's repo/i)).toBeInTheDocument();
  });

  it("edits a skill in place without deleting it", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));

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
    await openSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));
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

  it("cancel in the editor returns to the reading view unchanged", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));
    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));

    expect(screen.getByRole("heading", { name: "Steps" })).toBeInTheDocument();
    expect(ipc.saveSkill).not.toHaveBeenCalled();
  });

  it("shows the backend's message when a save is rejected", async () => {
    ipc.saveSkill.mockRejectedValueOnce(
      "Use a name like `release-notes` — lowercase letters, digits, and hyphens.",
    );
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    await userEvent.type(screen.getByPlaceholderText("release-notes"), "bad");
    await userEvent.type(screen.getByPlaceholderText(/writes release notes from the git log/i), "d");
    await userEvent.type(screen.getByLabelText("Instructions markdown"), "i");
    await userEvent.click(screen.getByRole("button", { name: "Add skill" }));
    expect(await screen.findByText(/use a name like/i)).toBeInTheDocument();
  });

  it("deletes a skill from the editor after a second confirming click, landing on the list", async () => {
    render(<SkillsPage />);
    await openSkill("review-checklist");
    await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));

    await userEvent.click(screen.getByRole("button", { name: /delete skill/i }));
    expect(ipc.deleteSkill).not.toHaveBeenCalled();

    ipc.listSkills.mockResolvedValue([releaseNotes]);
    await userEvent.click(screen.getByRole("button", { name: /really delete/i }));
    await waitFor(() =>
      expect(ipc.deleteSkill).toHaveBeenCalledWith("project", "review-checklist"),
    );
    expect(await screen.findByRole("button", { name: /new skill/i })).toBeInTheDocument();
  });
});
