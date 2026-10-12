import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { StartProjectModal } from "./StartProjectModal";
import * as ipc from "../../lib/ipc";
import { resetAll } from "../../test/utils";

function open() {
  const onCreated = vi.fn();
  const view = render(<StartProjectModal onClose={vi.fn()} onCreated={onCreated} />);
  return { ...view, onCreated };
}

describe("StartProjectModal", () => {
  beforeEach(() => resetAll());

  it("asks where the project lives before what it is called", () => {
    const { container } = open();
    const location = container.querySelector(".project-location");
    const name = screen.getByLabelText("Project name");
    expect(location).toBeTruthy();
    expect(location!.compareDocumentPosition(name) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("previews the folder it will create as the name is typed", async () => {
    vi.mocked(ipc.getDefaultProjectLocation).mockResolvedValue("/Users/me/Code/Apps");
    const { container, onCreated } = open();
    await waitFor(() => expect(container.querySelector(".project-location-path")?.textContent).toContain("~/Code/Apps/"));
    expect(screen.getByText("Default location")).toBeTruthy();

    await userEvent.type(screen.getByLabelText("Project name"), "Demo App");
    expect(container.querySelector(".project-location-leaf")?.textContent).toBe("Demo App");

    await userEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(ipc.startProject).toHaveBeenCalledWith({
      name: "Demo App",
      description: "",
      directory: "/Users/me/Code/Apps",
      createDirectory: true,
    });
  });

  it("names an existing folder's project after the folder until you rename it", async () => {
    vi.mocked(ipc.pickFolder).mockResolvedValueOnce("/work/oxen-site").mockResolvedValueOnce("/work/other");
    open();
    await userEvent.click(screen.getByRole("radio", { name: /Existing folder/ }));
    await userEvent.click(screen.getByRole("button", { name: "Choose project folder" }));
    await waitFor(() => expect(screen.getByLabelText("Project name")).toHaveValue("oxen-site"));

    await userEvent.clear(screen.getByLabelText("Project name"));
    await userEvent.type(screen.getByLabelText("Project name"), "Site");
    await userEvent.click(screen.getByRole("button", { name: "Choose project folder" }));
    await waitFor(() => expect(ipc.pickFolder).toHaveBeenCalledTimes(2));
    expect(screen.getByLabelText("Project name")).toHaveValue("Site");
  });

  it("keeps macOS text services out of the goal box, as in the chat composer", () => {
    open();
    const goal = screen.getByLabelText(/Goal/);
    expect(goal).toHaveAttribute("spellcheck", "false");
    expect(goal).toHaveAttribute("autocorrect", "off");
    expect(goal).toHaveAttribute("autocapitalize", "off");
    expect(goal).toHaveAttribute("writingsuggestions", "false");
  });
});
