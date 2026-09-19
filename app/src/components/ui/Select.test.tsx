import { expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FileText, Workflow } from "lucide-react";
import { Select } from "./Select";

const options = [
  {
    value: "file",
    label: "File",
    description: "Read and edit project files.",
    icon: <FileText />,
  },
  { value: "disabled", label: "Unavailable", disabled: true },
  {
    value: "workflow",
    label: "Oxen workflow",
    description: "Connect prompts, images, and video.",
    icon: <Workflow />,
  },
];

it("shows descriptions, icons and the current choice in a portalled menu", async () => {
  const change = vi.fn();
  const { container } = render(
    <Select
      label="Work view"
      value="file"
      options={options}
      onValueChange={change}
    />,
  );
  const trigger = screen.getByRole("combobox", { name: "Work view" });
  expect(trigger).toHaveTextContent("File");
  await userEvent.click(trigger);
  const menu = screen.getByRole("listbox", { name: "Work view" });
  expect(container).not.toContainElement(menu);
  const current = screen.getByRole("option", { name: "File" });
  expect(current).toHaveAttribute("aria-selected", "true");
  expect(current).toHaveAccessibleDescription("Read and edit project files.");
  expect(current.querySelector("svg")).not.toBeNull();
  await userEvent.click(screen.getByRole("option", { name: "Oxen workflow" }));
  expect(change).toHaveBeenCalledTimes(1);
  expect(change).toHaveBeenCalledWith("workflow");
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
});

it("supports arrows, Home/End, typeahead and Escape without changing the value", async () => {
  const change = vi.fn();
  render(
    <Select
      label="Work view"
      value="file"
      options={options}
      onValueChange={change}
    />,
  );
  const trigger = screen.getByRole("combobox");
  trigger.focus();
  await userEvent.keyboard("{ArrowDown}");
  expect(trigger).toHaveAttribute(
    "aria-activedescendant",
    screen.getByRole("option", { name: "File" }).id,
  );
  await userEvent.keyboard("{ArrowDown}");
  expect(trigger).toHaveAttribute(
    "aria-activedescendant",
    screen.getByRole("option", { name: "Oxen workflow" }).id,
  );
  await userEvent.keyboard("{Home}");
  expect(trigger).toHaveAttribute(
    "aria-activedescendant",
    screen.getByRole("option", { name: "File" }).id,
  );
  await userEvent.keyboard("ox");
  expect(trigger).toHaveAttribute(
    "aria-activedescendant",
    screen.getByRole("option", { name: "Oxen workflow" }).id,
  );
  await userEvent.keyboard("{Home}{End}{Enter}");
  expect(change).toHaveBeenCalledTimes(1);
  expect(change).toHaveBeenCalledWith("workflow");
  await userEvent.keyboard("{ArrowUp}{Escape}");
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
  expect(change).toHaveBeenCalledTimes(1);
});

it("dismisses on outside clicks and tabs without stealing the next control's focus", async () => {
  render(
    <>
      <Select
        label="Work view"
        value="file"
        options={options}
        onValueChange={vi.fn()}
      />
      <button>Next control</button>
    </>,
  );
  const trigger = screen.getByRole("combobox");
  await userEvent.click(trigger);
  await userEvent.tab();
  expect(screen.getByRole("button", { name: "Next control" })).toHaveFocus();
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  await userEvent.click(trigger);
  await userEvent.click(screen.getByRole("button", { name: "Next control" }));
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
});

it("does not open disabled or empty selectors and closes when disabled", async () => {
  const { rerender } = render(
    <Select
      label="Work view"
      value="file"
      options={options}
      onValueChange={vi.fn()}
    />,
  );
  await userEvent.click(screen.getByRole("combobox"));
  rerender(
    <Select
      label="Work view"
      value="file"
      options={options}
      onValueChange={vi.fn()}
      disabled
    />,
  );
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  expect(screen.getByRole("combobox")).toBeDisabled();
  rerender(
    <Select label="Work view" value="" options={[]} onValueChange={vi.fn()} />,
  );
  expect(screen.getByRole("combobox")).toBeDisabled();
});
