import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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
  draft: false,
};

/** A minimal ToolInfo for the autocomplete vocabulary and palette. */
const tool = (name: string, description = "", enabled = true) => ({
  name,
  description,
  default_description: description,
  parameters: {},
  enabled,
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
  draft: false,
};

/** A freshly created skill, as the backend lists it: an empty draft. */
const deployDraft: SkillInfo = {
  name: "deploy-steps",
  description: "",
  instructions: "",
  scope: "project",
  dir: "/repo/.oxen-harness/skills/deploy-steps",
  enabled: true,
  draft: true,
};

beforeEach(() => {
  resetAll();
  // An active project so scope labels can name it.
  useStore.setState({
    session: { ...ipc.sampleSession, workspace: "/repo" },
    projects: [{ path: "/repo", name: "OxenHarness", description: "", instructions: "", context: [], remote_repo: null, session_count: 1, active: true, last_used_at: null }],
  });
  ipc.listSkills.mockResolvedValue([releaseNotes, reviewChecklist]);
  ipc.listTools.mockResolvedValue([
    tool("read_file", "Read a UTF-8 text file."),
    tool("run_shell", "Run a shell command from the workspace root."),
    tool("web_search", "Search the web with Brave.", false),
  ]);
});

/** Open a skill's reading view from the list. */
async function openSkill(name: string) {
  await userEvent.click(await screen.findByRole("button", { name: `Open skill ${name}` }));
}

/** Open a skill's editor from the list. */
async function editSkill(name: string) {
  await openSkill(name);
  await userEvent.click(screen.getByRole("button", { name: /^edit$/i }));
}

describe("SkillsPage", () => {
  it("places itself in the tools/skills/rules trio and navigates between them", async () => {
    render(<SkillsPage />);
    await screen.findByText("release-notes");

    // All three ways of teaching the agent are named, with this one current.
    expect(screen.getByRole("button", { name: /Skills what it knows how to do/ })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await userEvent.click(screen.getByRole("button", { name: /Tools what it can do/ }));
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

  it("creates a skill from just a name and scope, then opens it in the editor", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));

    // The intro screen asks only for identity; the instructions come next.
    expect(screen.queryByLabelText("Instructions markdown")).toBeNull();
    expect(screen.getByRole("button", { name: "Create skill" })).toBeDisabled();
    await userEvent.type(screen.getByPlaceholderText("release-notes"), "deploy-steps");
    await userEvent.click(screen.getByRole("combobox", { name: "Skill scope" }));
    await userEvent.click(screen.getByRole("option", { name: /^This project only/ }));

    ipc.listSkills.mockResolvedValue([deployDraft, releaseNotes, reviewChecklist]);
    await userEvent.click(screen.getByRole("button", { name: "Create skill" }));

    expect(ipc.createSkill).toHaveBeenCalledWith("project", "deploy-steps");
    expect(await screen.findByLabelText("Instructions markdown")).toBeInTheDocument();
    expect(screen.getByText(/model won't see this skill/i)).toBeInTheDocument();
  });

  it("shows the backend's message when creating is rejected", async () => {
    ipc.createSkill.mockRejectedValueOnce(
      "A skill named `release-notes` already exists in every project.",
    );
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    await userEvent.type(screen.getByPlaceholderText("release-notes"), "release-notes");
    await userEvent.click(screen.getByRole("button", { name: "Create skill" }));

    expect(await screen.findByText(/already exists/i)).toBeInTheDocument();
    expect(screen.queryByLabelText("Instructions markdown")).toBeNull();
  });

  it("marks drafts in the list", async () => {
    ipc.listSkills.mockResolvedValue([deployDraft, releaseNotes]);
    render(<SkillsPage />);
    expect(await screen.findByText("draft")).toBeInTheDocument();
  });

  it("names the active project in the scope choice", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new skill/i }));
    await userEvent.click(screen.getByRole("combobox", { name: "Skill scope" }));
    const choice = screen.getByRole("option", { name: "This project only — OxenHarness" });

    // Choosing it makes the hint say exactly where the skill will land.
    await userEvent.click(choice);
    expect(screen.getByText(/saved into OxenHarness's repo/i)).toBeInTheDocument();
  });

  it("previews the instructions as markdown while editing", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");

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
    await editSkill("release-notes");

    const editor = screen.getByLabelText<HTMLTextAreaElement>("Instructions markdown");
    await userEvent.type(editor, "\nThen run `run");

    // The suggestion list offers the matching tool; accepting completes the
    // reference with its closing backtick.
    await userEvent.click(screen.getByRole("option", { name: "run_shell" }));
    expect(editor.value).toContain("Then run `run_shell`");

    // The reference summary now lists it as a known tool.
    expect(screen.getByLabelText("Referenced tools")).toHaveTextContent("run_shell");
  });

  it("browses the tool palette and inserts a reference from it", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");

    await userEvent.click(screen.getByRole("button", { name: /available tools/i }));
    // The palette lists every tool with its description; disabled ones are marked.
    expect(screen.getByText("Run a shell command from the workspace root.")).toBeInTheDocument();
    expect(screen.getByText("web_search").closest(".tool-palette-row")).toHaveClass("disabled");

    await userEvent.click(screen.getByRole("button", { name: /run_shell/ }));
    const editor = screen.getByLabelText<HTMLTextAreaElement>("Instructions markdown");
    expect(editor.value).toContain("`run_shell`");
    // Inserting closes the palette.
    expect(screen.queryByText(/everything the agent can call/i)).toBeNull();
  });

  it("warns about backticked names that don't match any tool", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");

    const editor = screen.getByLabelText<HTMLTextAreaElement>("Instructions markdown");
    await userEvent.type(editor, "\nUse `run_shel` here.");

    const warning = document.querySelector(".skill-ref-chip.unknown");
    expect(warning?.textContent).toContain("run_shel");
  });

  it("saves the instructions as soon as focus leaves the field", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");

    await userEvent.type(screen.getByLabelText("Instructions markdown"), "\n3. Ship it.");
    expect(ipc.saveSkill).not.toHaveBeenCalled();
    await userEvent.tab();

    await waitFor(() =>
      expect(ipc.saveSkill).toHaveBeenCalledWith(
        "global",
        "release-notes",
        releaseNotes.description,
        releaseNotes.instructions + "\n3. Ship it.",
      ),
    );
    expect(await screen.findByText("All changes saved")).toBeInTheDocument();
    expect(ipc.deleteSkill).not.toHaveBeenCalled();
  });

  it("saves pending typing when leaving the editor, landing on the reading view", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");

    const description = screen.getByDisplayValue("Writes release notes in our house style.");
    await userEvent.clear(description);
    await userEvent.type(description, "Better.");
    ipc.listSkills.mockResolvedValue([{ ...releaseNotes, description: "Better." }, reviewChecklist]);
    await userEvent.click(screen.getByRole("button", { name: "Done" }));

    await waitFor(() =>
      expect(ipc.saveSkill).toHaveBeenLastCalledWith(
        "global",
        "release-notes",
        "Better.",
        releaseNotes.instructions,
      ),
    );
    // The reading view shows the saved text, not the stale list entry.
    expect(await screen.findByRole("heading", { name: "Steps" })).toBeInTheDocument();
    expect(screen.getByText("Better.")).toBeInTheDocument();
  });

  it("leaving the editor untouched writes nothing", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");
    await userEvent.click(screen.getByRole("button", { name: "Done" }));

    expect(await screen.findByRole("heading", { name: "Steps" })).toBeInTheDocument();
    expect(ipc.saveSkill).not.toHaveBeenCalled();
  });

  it("keeps the user in the editor with the error when a save fails", async () => {
    ipc.saveSkill.mockRejectedValue("disk full");
    render(<SkillsPage />);
    await editSkill("release-notes");

    await userEvent.type(screen.getByLabelText("Instructions markdown"), "!");
    await userEvent.click(screen.getByRole("button", { name: "Done" }));

    expect(await screen.findByText("Couldn't save: disk full")).toBeInTheDocument();
    expect(screen.getByLabelText("Instructions markdown")).toBeInTheDocument();
  });

  it("renames and moves a skill, and later saves go to its new home", async () => {
    render(<SkillsPage />);
    await editSkill("release-notes");

    await userEvent.click(screen.getByRole("button", { name: /rename or move/i }));
    const form = screen.getByRole("form", { name: "Rename or move skill" });
    const name = within(form).getByDisplayValue("release-notes");
    await userEvent.clear(name);
    await userEvent.type(name, "changelog");
    await userEvent.click(within(form).getByRole("combobox", { name: "Skill scope" }));
    await userEvent.click(screen.getByRole("option", { name: /^This project only/ }));

    const moved: SkillInfo = {
      ...releaseNotes,
      name: "changelog",
      scope: "project",
      dir: "/repo/.oxen-harness/skills/changelog",
    };
    ipc.listSkills.mockResolvedValue([moved, reviewChecklist]);
    await userEvent.click(within(form).getByRole("button", { name: "Apply" }));

    await waitFor(() =>
      expect(ipc.renameSkill).toHaveBeenCalledWith(
        "global",
        "release-notes",
        "project",
        "changelog",
      ),
    );
    expect(await screen.findByRole("heading", { name: /changelog/ })).toBeInTheDocument();

    await userEvent.type(screen.getByLabelText("Instructions markdown"), "!");
    await userEvent.tab();
    await waitFor(() =>
      expect(ipc.saveSkill).toHaveBeenCalledWith(
        "project",
        "changelog",
        releaseNotes.description,
        releaseNotes.instructions + "!",
      ),
    );
    expect(ipc.saveSkill).not.toHaveBeenCalledWith(
      "global",
      "release-notes",
      expect.anything(),
      expect.anything(),
    );
  });

  it("imports a skill file into the chosen scope and opens it", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /import skill/i }));
    await userEvent.click(screen.getByRole("combobox", { name: "Skill scope" }));
    await userEvent.click(screen.getByRole("option", { name: /^This project only/ }));

    ipc.pickSkillFile.mockResolvedValue("/Downloads/deploy-steps.zip");
    ipc.importSkill.mockResolvedValue("deploy-steps");
    const imported = { ...deployDraft, description: "Deploys.", instructions: "Run it.", draft: false };
    ipc.listSkills.mockResolvedValue([imported, releaseNotes]);
    await userEvent.click(screen.getByRole("button", { name: /choose file/i }));

    expect(ipc.importSkill).toHaveBeenCalledWith("project", "/Downloads/deploy-steps.zip");
    expect(await screen.findByText("Run it.")).toBeInTheDocument();
  });

  it("stays on the import screen when the picker is cancelled or the file is rejected", async () => {
    render(<SkillsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /import skill/i }));

    await userEvent.click(screen.getByRole("button", { name: /choose file/i }));
    expect(ipc.importSkill).not.toHaveBeenCalled();

    ipc.pickSkillFile.mockResolvedValue("/Downloads/notes.zip");
    ipc.importSkill.mockRejectedValue("notes.zip isn't a skill archive: no SKILL.md inside");
    await userEvent.click(screen.getByRole("button", { name: /choose file/i }));
    expect(await screen.findByText(/no SKILL.md inside/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /choose file/i })).toBeEnabled();
  });

  it("exports a skill as a zip from its reading view", async () => {
    render(<SkillsPage />);
    await openSkill("release-notes");

    ipc.pickSkillExportPath.mockResolvedValue("/Downloads/release-notes.zip");
    await userEvent.click(screen.getByRole("button", { name: /export/i }));

    expect(ipc.pickSkillExportPath).toHaveBeenCalledWith("release-notes");
    expect(ipc.exportSkill).toHaveBeenCalledWith(
      "global",
      "release-notes",
      "/Downloads/release-notes.zip",
    );
    expect(await screen.findByText("/Downloads/release-notes.zip")).toBeInTheDocument();
  });

  it("deletes a skill from the editor after a second confirming click, landing on the list", async () => {
    render(<SkillsPage />);
    await editSkill("review-checklist");

    await userEvent.click(screen.getByRole("button", { name: /delete skill/i }));
    expect(ipc.deleteSkill).not.toHaveBeenCalled();

    ipc.listSkills.mockResolvedValue([releaseNotes]);
    await userEvent.click(screen.getByRole("button", { name: /really delete/i }));
    await waitFor(() =>
      expect(ipc.deleteSkill).toHaveBeenCalledWith("project", "review-checklist"),
    );
    expect(await screen.findByRole("button", { name: /new skill/i })).toBeInTheDocument();
    // Nothing writes the deleted skill back on the way out.
    expect(ipc.saveSkill).not.toHaveBeenCalled();
  });
});
