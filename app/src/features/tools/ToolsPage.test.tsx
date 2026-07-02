import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ToolsPage } from "./ToolsPage";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { ToolInfo } from "../../lib/types";

const builtin: ToolInfo = {
  name: "read_file",
  description: "Read a file from the workspace.",
  default_description: "Read a file from the workspace.",
  parameters: { type: "object", properties: {} },
  enabled: true,
  builtin: true,
  config: {},
};

const custom: ToolInfo = {
  name: "lookup_customer",
  description: "Look up a customer by email.",
  default_description: "Look up a customer by email.",
  parameters: {
    type: "object",
    properties: { email: { type: "string", description: "Customer email" } },
    required: ["email"],
  },
  enabled: true,
  builtin: false,
  config: { type: "HTTP POST", url: "https://api.example.com/lookup" },
};

beforeEach(() => {
  resetAll();
  ipc.listTools.mockResolvedValue([builtin, custom]);
});

describe("ToolsPage", () => {
  it("splits tools into custom and built-in sections", async () => {
    render(<ToolsPage />);
    expect(await screen.findByText("lookup_customer")).toBeInTheDocument();
    expect(screen.getByText("read_file")).toBeInTheDocument();
    expect(screen.getByText(/your tools/i)).toBeInTheDocument();
    expect(screen.getByText(/built-in tools/i)).toBeInTheDocument();
  });

  it("adds a new tool through the simple editor", async () => {
    render(<ToolsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new tool/i }));

    await userEvent.type(screen.getByPlaceholderText("lookup_customer"), "get_weather");
    await userEvent.type(
      screen.getByPlaceholderText(/what does this tool do/i),
      "Get the weather for a city.",
    );
    await userEvent.type(
      screen.getByPlaceholderText("https://api.example.com/lookup"),
      "https://api.example.com/weather",
    );
    // Fill the starter parameter row.
    await userEvent.type(screen.getByLabelText("Parameter 1 name"), "city");
    await userEvent.type(screen.getByLabelText("Parameter 1 description"), "City name");
    await userEvent.click(screen.getByLabelText("Parameter 1 required"));

    await userEvent.click(screen.getByRole("button", { name: "Add tool" }));

    await waitFor(() =>
      expect(ipc.addCustomTool).toHaveBeenCalledWith({
        name: "get_weather",
        description: "Get the weather for a city.",
        parameters: {
          type: "object",
          properties: { city: { type: "string", description: "City name" } },
          required: ["city"],
        },
        action: { kind: "http_post", url: "https://api.example.com/weather" },
      }),
    );
    // The editor closes back to the add button.
    expect(screen.getByRole("button", { name: /new tool/i })).toBeInTheDocument();
  });

  it("won't save a new tool until name, description, and URL are filled", async () => {
    render(<ToolsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new tool/i }));
    expect(screen.getByRole("button", { name: "Add tool" })).toBeDisabled();
  });

  it("shows the backend's message when a spec is rejected", async () => {
    ipc.addCustomTool.mockRejectedValueOnce("`read_file` is a built-in tool name. Choose a unique name.");
    render(<ToolsPage />);
    await userEvent.click(await screen.findByRole("button", { name: /new tool/i }));
    await userEvent.type(screen.getByPlaceholderText("lookup_customer"), "read_file");
    await userEvent.type(screen.getByPlaceholderText(/what does this tool do/i), "d");
    await userEvent.type(screen.getByPlaceholderText("https://api.example.com/lookup"), "https://x.dev");
    await userEvent.click(screen.getByRole("button", { name: "Add tool" }));
    expect(await screen.findByText(/is a built-in tool name/i)).toBeInTheDocument();
  });

  it("edits an existing custom tool in place", async () => {
    render(<ToolsPage />);
    await userEvent.click(await screen.findByText("lookup_customer"));

    const url = screen.getByDisplayValue("https://api.example.com/lookup");
    await userEvent.clear(url);
    await userEvent.type(url, "https://api.example.com/v2/lookup");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(ipc.addCustomTool).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "lookup_customer",
          action: { kind: "http_post", url: "https://api.example.com/v2/lookup" },
        }),
      ),
    );
    expect(ipc.removeCustomTool).not.toHaveBeenCalled();
  });

  it("renames by adding the new name first, then removing the old", async () => {
    render(<ToolsPage />);
    await userEvent.click(await screen.findByText("lookup_customer"));

    const name = screen.getByDisplayValue("lookup_customer");
    await userEvent.clear(name);
    await userEvent.type(name, "find_customer");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(ipc.removeCustomTool).toHaveBeenCalledWith("lookup_customer"));
    expect(ipc.addCustomTool).toHaveBeenCalledWith(expect.objectContaining({ name: "find_customer" }));
  });

  it("deletes a custom tool only after a second confirming click", async () => {
    render(<ToolsPage />);
    await userEvent.click(await screen.findByText("lookup_customer"));

    await userEvent.click(screen.getByRole("button", { name: /delete tool/i }));
    expect(ipc.removeCustomTool).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: /really delete/i }));
    await waitFor(() => expect(ipc.removeCustomTool).toHaveBeenCalledWith("lookup_customer"));
  });

  it("toggles a custom tool like any other", async () => {
    render(<ToolsPage />);
    await screen.findByText("lookup_customer");
    await userEvent.click(screen.getByLabelText("Disable lookup_customer"));
    expect(ipc.setToolEnabled).toHaveBeenCalledWith("lookup_customer", false);
  });

  it("edits parameters as raw JSON and round-trips back to simple mode", async () => {
    render(<ToolsPage />);
    await userEvent.click(await screen.findByText("lookup_customer"));

    await userEvent.click(screen.getByRole("button", { name: "JSON" }));
    const editor = screen.getByLabelText<HTMLTextAreaElement>("Parameters JSON Schema");
    expect(editor.value).toContain("email");

    // Broken JSON can't switch back to the simple editor.
    await userEvent.clear(editor);
    await userEvent.type(editor, "not json");
    await userEvent.click(screen.getByRole("button", { name: "Simple" }));
    expect(screen.getByText(/fix the json/i)).toBeInTheDocument();
  });
});
