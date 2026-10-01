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

it("keeps an active option when the available choices change while open", async () => {
  const change = vi.fn();
  const { rerender } = render(
    <Select
      label="Work view"
      value="file"
      options={options}
      onValueChange={change}
    />,
  );
  const trigger = screen.getByRole("combobox");
  await userEvent.click(trigger);
  rerender(
    <Select
      label="Work view"
      value="file"
      options={[options[1], options[2]]}
      onValueChange={change}
    />,
  );
  const next = screen.getByRole("option", { name: "Oxen workflow" });
  expect(trigger).toHaveAttribute("aria-activedescendant", next.id);
  await userEvent.keyboard("{Enter}");
  expect(change).toHaveBeenCalledWith("workflow");
});

it("filters a searchable menu as you type and picks the match with Enter", async () => {
  const change = vi.fn();
  render(
    <>
      <Select
        label="Work view"
        value="file"
        options={[...options, { value: "priced", label: "Priced", hint: "$0.04" }]}
        onValueChange={change}
        searchable
      />
      <button>Next control</button>
    </>,
  );
  const trigger = screen.getByRole("combobox");
  expect(trigger).not.toHaveTextContent("$0.04");
  await userEvent.click(trigger);
  const search = screen.getByRole("searchbox", { name: "Search Work view" });
  expect(search).toHaveFocus();
  expect(screen.getByRole("option", { name: "Priced" })).toHaveTextContent("$0.04");

  // Words match anywhere in the label or description, in any order.
  await userEvent.keyboard("video prompts");
  expect(screen.getAllByRole("option")).toHaveLength(1);
  expect(search).toHaveAttribute(
    "aria-activedescendant",
    screen.getByRole("option", { name: "Oxen workflow" }).id,
  );

  await userEvent.clear(search);
  await userEvent.keyboard("zzz");
  expect(screen.queryByRole("option")).not.toBeInTheDocument();
  expect(screen.getByText("No matches")).toBeInTheDocument();
  await userEvent.keyboard("{Enter}");
  expect(change).not.toHaveBeenCalled();

  await userEvent.clear(search);
  await userEvent.keyboard("ox{Enter}");
  expect(change).toHaveBeenCalledWith("workflow");
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();

  // Reopening starts from a clean query, and Tab dismisses the menu.
  await userEvent.click(trigger);
  expect(screen.getByRole("searchbox")).toHaveValue("");
  await userEvent.tab();
  expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
});
