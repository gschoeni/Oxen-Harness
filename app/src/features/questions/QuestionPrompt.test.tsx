import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { QuestionPrompt } from "./QuestionPrompt";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { QuestionPayload } from "../../lib/types";

const single: QuestionPayload = {
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
  useStore.setState({ question: single });
});

describe("QuestionPrompt", () => {
  it("renders the current question and its options", () => {
    useStore.setState({ question: single });
    render(<QuestionPrompt />);
    expect(screen.getByText("Which database should we use?")).toBeInTheDocument();
    expect(screen.getByText("Postgres")).toBeInTheDocument();
    expect(screen.getByText("SQLite")).toBeInTheDocument();
    expect(screen.getByText("Database")).toBeInTheDocument();
  });

  it("single-select commits on click and clears the prompt", async () => {
    render(<QuestionPrompt />);
    await userEvent.click(screen.getByText("SQLite"));
    expect(ipc.answerQuestion).toHaveBeenCalledWith("q0", [
      { header: "Database", question: "Which database should we use?", selected: ["SQLite"] },
    ]);
    expect(useStore.getState().question).toBeNull();
  });

  it("submits free-text via the Other input", async () => {
    render(<QuestionPrompt />);
    await userEvent.type(screen.getByPlaceholderText(/type your own answer/i), "DuckDB{Enter}");
    expect(ipc.answerQuestion).toHaveBeenCalledWith("q0", [
      expect.objectContaining({ selected: ["DuckDB"] }),
    ]);
  });

  it("asks multiple questions one at a time, then submits all answers", async () => {
    useStore.setState({
      question: {
        id: "q1",
        questions: [
          {
            question: "What kind of website?",
            header: "Type",
            multiSelect: false,
            options: [
              { label: "Landing page", description: "" },
              { label: "Docs", description: "" },
            ],
          },
          {
            question: "Which features?",
            header: "Features",
            multiSelect: true,
            options: [
              { label: "Auth", description: "" },
              { label: "Search", description: "" },
            ],
          },
        ],
      },
    });
    render(<QuestionPrompt />);

    // First (single-select) question shows alone; picking advances.
    expect(screen.getByText("What kind of website?")).toBeInTheDocument();
    expect(screen.queryByText("Which features?")).toBeNull();
    await userEvent.click(screen.getByText("Landing page"));

    // Now the second (multi-select) question; the agent isn't answered yet.
    expect(screen.getByText("Which features?")).toBeInTheDocument();
    expect(ipc.answerQuestion).not.toHaveBeenCalled();

    await userEvent.click(screen.getByText("Auth"));
    await userEvent.click(screen.getByText("Search"));
    await userEvent.click(screen.getByRole("button", { name: /submit/i }));

    expect(ipc.answerQuestion).toHaveBeenCalledWith("q1", [
      { header: "Type", question: "What kind of website?", selected: ["Landing page"] },
      { header: "Features", question: "Which features?", selected: ["Auth", "Search"] },
    ]);
    expect(useStore.getState().question).toBeNull();
  });
});
