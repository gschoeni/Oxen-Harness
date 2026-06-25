import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { QuestionModal } from "./QuestionModal";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { QuestionPayload } from "../../lib/types";

const payload: QuestionPayload = {
  id: "q0",
  questions: [
    {
      question: "Which database should we use?",
      header: "Database",
      multiSelect: false,
      options: [
        { label: "Postgres", description: "Relational, battle-tested" },
        { label: "SQLite", description: "Embedded, zero-config" },
      ],
    },
  ],
};

beforeEach(() => {
  resetAll();
  useStore.setState({ question: payload });
});

describe("QuestionModal", () => {
  it("renders the question and its options", () => {
    render(<QuestionModal />);
    expect(screen.getByText("Which database should we use?")).toBeInTheDocument();
    expect(screen.getByText("Postgres")).toBeInTheDocument();
    expect(screen.getByText("SQLite")).toBeInTheDocument();
    expect(screen.getByText("Database")).toBeInTheDocument();
  });

  it("submits the (default-selected) answer and clears the question", async () => {
    render(<QuestionModal />);
    await userEvent.click(screen.getByRole("button", { name: /send answer/i }));
    expect(ipc.answerQuestion).toHaveBeenCalledWith("q0", [
      { header: "Database", question: "Which database should we use?", selected: ["Postgres"] },
    ]);
    expect(useStore.getState().question).toBeNull();
  });

  it("lets the user pick a different option", async () => {
    render(<QuestionModal />);
    await userEvent.click(screen.getByText("SQLite"));
    await userEvent.click(screen.getByRole("button", { name: /send answer/i }));
    expect(ipc.answerQuestion).toHaveBeenCalledWith("q0", [
      expect.objectContaining({ selected: ["SQLite"] }),
    ]);
  });

  it("includes free-text 'Other' input in the answer", async () => {
    render(<QuestionModal />);
    await userEvent.type(screen.getByPlaceholderText(/type your own answer/i), "DuckDB");
    await userEvent.click(screen.getByRole("button", { name: /send answer/i }));
    expect(ipc.answerQuestion).toHaveBeenCalledWith("q0", [
      expect.objectContaining({ selected: ["Postgres", "DuckDB"] }),
    ]);
  });
});
