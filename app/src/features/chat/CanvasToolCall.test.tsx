import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ToolCall } from "./ToolCall";
import { resetAll } from "../../test/utils";
import type { Item } from "./thread";

type ToolItem = Extract<Item, { kind: "tool" }>;

const canvas = (over: Partial<ToolItem>): ToolItem => ({
  kind: "tool",
  id: "c1",
  name: "canvas",
  args: "",
  result: "",
  running: false,
  startedAt: 0,
  endedAt: 0,
  ...over,
} as ToolItem);

beforeEach(() => {
  resetAll();
});

describe("a canvas call in the thread", () => {
  it("opens the document when the call succeeded", () => {
    render(
      <ToolCall
        item={canvas({ args: JSON.stringify({ format: "markdown", title: "Report", content: "# hi" }), result: "shown" })}
      />,
    );
    expect(screen.getByText("Report")).toBeInTheDocument();
    expect(screen.getByText("open ›")).toBeInTheDocument();
  });

  it("reads as the error it was when the tool refused the call", () => {
    // Seen in the wild: a call that arrived with no arguments showed as
    // "Document · markdown · open ›" with nothing behind the link.
    render(<ToolCall item={canvas({ args: "", result: "tool error: invalid arguments: missing field `format`" })} />);
    expect(screen.queryByText("open ›")).not.toBeInTheDocument();
    expect(screen.queryByText("Document")).not.toBeInTheDocument();
    expect(document.querySelector(".toolcall.failed")).not.toBeNull();
  });
});
