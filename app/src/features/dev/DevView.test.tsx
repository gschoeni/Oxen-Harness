import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { DevView } from "./DevView";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const MESSAGES = [
  { role: "system", content: "You are a coding agent." },
  { role: "user", content: "List the files" },
  {
    role: "assistant",
    content: "Let me look.",
    tool_calls: [{ id: "call_1", type: "function", function: { name: "find_files", arguments: '{"glob":"**/*.rs"}' } }],
  },
  { role: "tool", tool_call_id: "call_1", content: "src/main.rs" },
  { role: "assistant", content: "Found one file: src/main.rs" },
];

beforeEach(() => {
  resetAll();
  useStore.setState({
    devViewOpen: true,
    session: { ...ipc.sampleSession, session_id: "s1", tokens_used: 1234, context_tokens: 900, context_window: 128000 },
  });
  ipc.sessionMessages.mockResolvedValue(MESSAGES as unknown[]);
  ipc.toolDefinitions.mockResolvedValue([
    { type: "function", function: { name: "find_files", description: "Locate files", parameters: {} } },
    { type: "function", function: { name: "read_file", description: "Read a file", parameters: {} } },
  ] as unknown[]);
});

describe("DevView", () => {
  it("fetches the session transcript and summarizes it", async () => {
    render(<DevView />);
    await waitFor(() => expect(ipc.sessionMessages).toHaveBeenCalledWith("s1"));
    // Messages count, LLM calls (2 assistant), tool calls (1)
    expect(await screen.findByText("Messages")).toBeInTheDocument();
    expect(screen.getByText("5")).toBeInTheDocument(); // message count
    expect(screen.getByText("1,234")).toBeInTheDocument(); // session tokens
    // Tool name shows in the summary chip (and again in the tool-call block)
    expect(screen.getAllByText("find_files").length).toBeGreaterThan(0);
  });

  it("surfaces the tool definitions panel", async () => {
    render(<DevView />);
    await waitFor(() => expect(ipc.toolDefinitions).toHaveBeenCalled());
    // The collapsible tool-definitions panel and its available count
    expect(await screen.findByText("Tool definitions")).toBeInTheDocument();
    expect(screen.getByText("2 available")).toBeInTheDocument();
    // Expanding lists each tool's name
    await userEvent.click(screen.getByText("Tool definitions"));
    expect(screen.getByText("read_file")).toBeInTheDocument();
  });

  it("documents the tool-definitions token estimate", async () => {
    render(<DevView />);
    await screen.findByText("Tool definitions");
    // Summary stat shows the count + token estimate, and the panel/rows show tok badges.
    expect(screen.getByText(/2 · ~\d/)).toBeInTheDocument();
    expect(screen.getAllByText(/~\d[\d,]* tok/).length).toBeGreaterThan(0);
  });

  it("renders role-coded messages and shows tool-call args", async () => {
    render(<DevView />);
    expect(await screen.findByText("User")).toBeInTheDocument();
    expect(screen.getAllByText("Assistant").length).toBe(2);
    expect(screen.getByText("Tool result")).toBeInTheDocument();
    // The tool call's JSON args are rendered
    expect(screen.getByText(/"glob"/)).toBeInTheDocument();
  });

  it("toggles to raw JSON view", async () => {
    render(<DevView />);
    await screen.findByText("User");
    await userEvent.click(screen.getByRole("tab", { name: /raw json/i }));
    // The raw dump contains the verbatim tool_call_id
    expect(screen.getByText(/"tool_call_id": "call_1"/)).toBeInTheDocument();
  });
});
