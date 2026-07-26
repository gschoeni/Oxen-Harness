import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { RulesPage } from "./RulesPage";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const rule = (over: Partial<Record<string, unknown>> = {}) => ({
  name: "no-unwrap",
  when: ".unwrap()",
  scope: ["tool"],
  message: "Return a Result instead.",
  interrupt: true,
  repeat: "once",
  enabled: true,
  ...over,
});

beforeEach(() => {
  resetAll();
});

describe("RulesPage", () => {
  it("explains what a rule does before showing its pattern, and adds it on click", async () => {
    render(<RulesPage />);

    // Plain language first — the regex is not what a new user needs to read.
    expect(await screen.findByText("Don't force-push")).toBeTruthy();
    expect(screen.getByText(/costs other people their work/)).toBeTruthy();
    expect(screen.getByText("git push --force")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: /Add no-force-push/ }));

    expect(ipc.saveRules).toHaveBeenCalledTimes(1);
    const saved = ipc.saveRules.mock.calls[0]?.[0] ?? [];
    expect(saved.map((r) => r.name)).toEqual(["no-force-push"]);
  });

  it("says the tester is waiting for a pattern rather than claiming no match", async () => {
    render(<RulesPage />);
    await userEvent.click(await screen.findByRole("button", { name: /New rule/ }));

    // An empty form hasn't been evaluated; "no match" would read as broken.
    expect(await screen.findByText(/add a pattern above/)).toBeTruthy();
    expect(screen.queryByText(/no match/)).toBeNull();
  });

  it("shows what a rule watches for and whether it interrupts", async () => {
    ipc.listRules.mockResolvedValue({
      user: [rule()],
      project: [],
      project_path: ".oxen-harness/rules.json",
    });

    render(<RulesPage />);

    expect(await screen.findByText(".unwrap()")).toBeTruthy();
    // The consequential distinction is legible without opening the rule.
    // Scoped to the list: suggestion cards carry the same label.
    const row = document.querySelector(".rule-row");
    expect(row?.textContent).toContain("interrupts");
    expect(row?.textContent).toContain("in tool calls");
  });

  it("previews the match and the reminder the model would receive", async () => {
    ipc.listRules.mockResolvedValue({
      user: [rule()],
      project: [],
      project_path: ".oxen-harness/rules.json",
    });

    render(<RulesPage />);
    await userEvent.click(await screen.findByRole("button", { name: /Edit no-unwrap/ }));

    // The default sample contains `.unwrap()`, so the rule fires…
    expect(await screen.findByText(/1 match — this rule would fire/)).toBeTruthy();
    expect(document.querySelector(".rule-hit")?.textContent).toBe(".unwrap()");
    // …and the page shows the exact reminder, plus what happens to the reply.
    expect(screen.getByText(/system-reminder rule="no-unwrap"/)).toBeTruthy();
    expect(screen.getByText(/thrown away/)).toBeTruthy();
  });

  it("won't save a rule that watches nothing", async () => {
    ipc.listRules.mockResolvedValue({
      user: [rule()],
      project: [],
      project_path: ".oxen-harness/rules.json",
    });

    render(<RulesPage />);
    await userEvent.click(await screen.findByRole("button", { name: /Edit no-unwrap/ }));
    // Turning off the only scope would mean "watch everything" downstream,
    // which is the opposite of what the user just asked for.
    await userEvent.click(screen.getByRole("button", { name: "tool calls", pressed: true }));

    expect(await screen.findByText(/Choose where it watches/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save rule" }).hasAttribute("disabled")).toBe(true);
  });

  it("never saves against a verdict from a previous pattern", async () => {
    ipc.listRules.mockResolvedValue({
      user: [rule()],
      project: [],
      project_path: ".oxen-harness/rules.json",
    });

    render(<RulesPage />);
    await userEvent.click(await screen.findByRole("button", { name: /Edit no-unwrap/ }));
    await screen.findByText(/1 match/);

    // Mid-edit, the old verdict must not still be gating the save.
    await userEvent.type(screen.getByDisplayValue(".unwrap()"), "(");
    expect(screen.getByRole("button", { name: "Save rule" }).hasAttribute("disabled")).toBe(true);
  });

  it("refuses to save a pattern the agent's engine rejects", async () => {
    ipc.listRules.mockResolvedValue({
      user: [rule()],
      project: [],
      project_path: ".oxen-harness/rules.json",
    });
    ipc.checkRulePattern.mockResolvedValue({
      error: "unclosed group",
      matches: [],
    });

    render(<RulesPage />);
    await userEvent.click(await screen.findByRole("button", { name: /Edit no-unwrap/ }));

    expect(await screen.findByText(/doesn't compile: unclosed group/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save rule" }).hasAttribute("disabled")).toBe(true);
  });

  it("shows the repository's own rules without letting you edit them here", async () => {
    ipc.listRules.mockResolvedValue({
      user: [],
      project: [rule({ name: "repo-rule" })],
      project_path: ".oxen-harness/rules.json",
    });

    render(<RulesPage />);

    expect(await screen.findByText("repo-rule")).toBeTruthy();
    expect(screen.getByText("repo")).toBeTruthy();
    // No edit affordance on a rule that lives in the repository.
    expect(screen.queryByRole("button", { name: /Edit repo-rule/ })).toBeNull();
  });
});
