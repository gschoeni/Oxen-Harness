import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Queue } from "./Queue";

describe("Queue", () => {
  it("renders nothing when empty", () => {
    const { container } = render(<Queue items={[]} onChange={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("lays out as one of the in-flight panels with a numbered row per message", () => {
    render(<Queue items={["first", "second"]} onChange={() => {}} />);
    const panel = screen.getByRole("status", { name: "Queued messages" });
    expect(panel).toHaveClass("fleet-panel");
    expect(screen.getByText("Queued · 2")).toHaveClass("fleet-panel-title");
    expect(screen.getByText("first").closest(".fleet-lane")).not.toBeNull();
    expect(screen.getByText("2")).toHaveClass("queue-idx");
  });

  it("removes one message or clears them all", () => {
    const onChange = vi.fn();
    render(<Queue items={["first", "second"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove queued message 1" }));
    expect(onChange).toHaveBeenLastCalledWith(["second"]);
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });
});
