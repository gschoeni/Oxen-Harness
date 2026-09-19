import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { ModelErrorNotice, errorReport, prettyDetail, statusName } from "./ModelError";
import { RetryPrompt } from "./RetryPrompt";
import { ThreadItem } from "./ThreadItem";
import { useStore } from "../../lib/store";
import { sampleSession } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { ModelErrorDetail } from "../../lib/types";

beforeEach(resetAll);

const error: ModelErrorDetail = {
  at: Date.UTC(2026, 8, 18, 11, 41, 2),
  error: "Oxen API error (502): The model provider returned an error.",
  model: "deepseek-v4-pro",
  endpoint: "https://hub.oxen.ai/api/ai",
  status: 502,
  detail: '{"error":{"type":"upstream","title":"The model provider returned an error."},"status":502}',
  attempt: 1,
  maxAttempts: 4,
  next: { kind: "retry", delayMs: 1000 },
};

describe("ModelErrorNotice", () => {
  it("renders the quiet one-liner as a button and opens the detail view on click", () => {
    render(
      <ModelErrorNotice
        text="Model call failed (Oxen API error (502): The model provider returned an error.) — retrying in 1s (attempt 2 of 4)"
        error={error}
      />,
    );
    const button = screen.getByRole("button", { name: /Model call failed/ });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(button);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Model call failed");
    // Where it went, what came back, and what the agent did next.
    expect(dialog).toHaveTextContent("deepseek-v4-pro");
    expect(dialog).toHaveTextContent("https://hub.oxen.ai/api/ai");
    expect(dialog).toHaveTextContent("502 Bad Gateway");
    expect(dialog).toHaveTextContent("1 of 4");
    expect(dialog).toHaveTextContent("retried after 1s");
    // The raw provider body is re-indented so its fields read.
    expect(dialog.querySelector(".model-error-body")?.textContent).toContain('"type": "upstream"');
    expect(dialog).toHaveTextContent("~/.oxen-harness/errors.jsonl");
  });

  it("says so when the provider sent no body", () => {
    render(<ModelErrorNotice text="failed" error={{ ...error, detail: undefined, status: undefined }} />);
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByRole("dialog")).toHaveTextContent(/No response body came back/);
    expect(screen.getByRole("dialog")).not.toHaveTextContent("Status");
  });

  it("is what a thread notice carrying an error renders as; plain notices stay inert", () => {
    render(<ThreadItem item={{ id: "n1", kind: "notice", text: "failed once", error }} />);
    expect(screen.getByRole("button", { name: /failed once/ })).toBeInTheDocument();

    render(<ThreadItem item={{ id: "n2", kind: "notice", text: "Compacted context — trimmed" }} />);
    expect(screen.queryByRole("button", { name: /Compacted/ })).not.toBeInTheDocument();
    expect(screen.getByText("Compacted context — trimmed")).toBeInTheDocument();
  });
});

describe("RetryPrompt error details", () => {
  it("opens the detail view with the turn's final error and the last failed call", () => {
    useStore.setState({ session: sampleSession });
    render(
      <RetryPrompt
        item={{
          id: "r1",
          kind: "retry",
          text: "hi",
          attachments: [],
          message:
            "the model endpoint failed 4 times in a row (deepseek-v4-pro at https://hub.oxen.ai/api/ai) — last error: Oxen API error (502): The model provider returned an error.",
          lastError: error,
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "View error details" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Turn ended");
    expect(dialog).toHaveTextContent("failed 4 times in a row");
    expect(dialog).toHaveTextContent("502 Bad Gateway");
    expect(dialog.querySelector(".model-error-body")?.textContent).toContain("upstream");
  });

  it("still opens for a turn that died without a retry, explaining the missing body", async () => {
    useStore.setState({ session: sampleSession });
    render(
      <RetryPrompt
        item={{ id: "r1", kind: "retry", text: "hi", attachments: [], message: "Oxen API error (402): out of credits" }}
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "View error details" }));
    });
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Oxen API error (402): out of credits");
    expect(dialog).toHaveTextContent(/failed before any call could be retried/);
  });
});

describe("error report helpers", () => {
  it("pretty-prints JSON bodies and leaves other text alone", () => {
    expect(prettyDetail('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyDetail("<html>502</html>")).toBe("<html>502</html>");
  });

  it("names the statuses providers return", () => {
    expect(statusName(502)).toBe("Bad Gateway");
    expect(statusName(529)).toBe("Overloaded");
    expect(statusName(418)).toBe("");
  });

  it("builds a paste-ready report with every known fact", () => {
    const report = errorReport(error, "the turn ended");
    expect(report).toContain("Turn ended: the turn ended");
    expect(report).toContain("Model: deepseek-v4-pro");
    expect(report).toContain("Endpoint: https://hub.oxen.ai/api/ai");
    expect(report).toContain("Status: 502 Bad Gateway");
    expect(report).toContain("Attempt: 1 of 4");
    expect(report).toContain("Then: retried after 1s");
    expect(report).toContain('"title": "The model provider returned an error."');
    expect(report).toContain("Log: ~/.oxen-harness/errors.jsonl");
  });
});
