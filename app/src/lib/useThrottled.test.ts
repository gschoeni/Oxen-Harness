import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useThrottled } from "./useThrottled";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useThrottled", () => {
  it("coalesces a burst of updates into one trailing emit of the last value", () => {
    const { result, rerender } = renderHook(({ v }) => useThrottled(v, 100), {
      initialProps: { v: "a" },
    });
    expect(result.current).toBe("a");

    rerender({ v: "b" });
    rerender({ v: "c" });
    expect(result.current).toBe("a");

    act(() => vi.advanceTimersByTime(100));
    expect(result.current).toBe("c");
  });

  it("keeps emitting once per interval while updates continue", () => {
    const { result, rerender } = renderHook(({ v }) => useThrottled(v, 100), {
      initialProps: { v: 1 },
    });
    rerender({ v: 2 });
    act(() => vi.advanceTimersByTime(100));
    expect(result.current).toBe(2);

    rerender({ v: 3 });
    rerender({ v: 4 });
    act(() => vi.advanceTimersByTime(100));
    expect(result.current).toBe(4);
  });

  it("does not schedule work when the value is stable", () => {
    const { result, rerender } = renderHook(({ v }) => useThrottled(v, 100), {
      initialProps: { v: "same" },
    });
    rerender({ v: "same" });
    act(() => vi.advanceTimersByTime(500));
    expect(result.current).toBe("same");
    expect(vi.getTimerCount()).toBe(0);
  });
});
