// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";

import { useAutosave } from "./useAutosave";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("useAutosave", () => {
  it("saves once, with the latest value, after typing pauses", async () => {
    const save = vi.fn(async (_: string) => {});
    const { result } = renderHook(() => useAutosave(save, 500));

    act(() => result.current.schedule("a"));
    await act(() => vi.advanceTimersByTimeAsync(300));
    act(() => result.current.schedule("ab"));
    await act(() => vi.advanceTimersByTimeAsync(300));
    expect(save).not.toHaveBeenCalled();
    expect(result.current.status).toBe("pending");

    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith("ab");
    expect(result.current.status).toBe("saved");
  });

  it("flush saves right away, and is a no-op with nothing pending", async () => {
    const save = vi.fn(async (_: string) => {});
    const { result } = renderHook(() => useAutosave(save, 500));

    await act(() => result.current.flush());
    expect(save).not.toHaveBeenCalled();

    act(() => result.current.schedule("x"));
    await act(() => result.current.flush());
    expect(save).toHaveBeenCalledWith("x");
    // The cancelled timer doesn't save a second time.
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("writes pending changes on unmount", async () => {
    const save = vi.fn(async (_: string) => {});
    const { result, unmount } = renderHook(() => useAutosave(save, 500));
    act(() => result.current.schedule("last words"));
    unmount();
    await vi.runAllTimersAsync();
    expect(save).toHaveBeenCalledWith("last words");
  });

  it("reports a failed save and retries it on the next flush", async () => {
    const save = vi.fn(async (_: string) => {});
    save.mockRejectedValueOnce("disk full");
    const { result } = renderHook(() => useAutosave(save, 500));

    act(() => result.current.schedule("x"));
    await act(() => expect(result.current.flush()).rejects.toBe("disk full"));
    expect(result.current.status).toBe("error");
    expect(result.current.error).toBe("disk full");

    await act(() => result.current.flush());
    expect(save).toHaveBeenLastCalledWith("x");
    expect(result.current.status).toBe("saved");
    expect(result.current.error).toBeNull();
  });

  it("runs saves and queued jobs strictly in order", async () => {
    const log: string[] = [];
    let release!: () => void;
    const save = vi.fn(async (v: string) => {
      if (v === "slow") await new Promise<void>((r) => (release = r));
      log.push(`save ${v}`);
    });
    const { result } = renderHook(() => useAutosave(save, 500));

    act(() => result.current.schedule("slow"));
    let flushed!: Promise<void>;
    act(() => {
      flushed = result.current.flush();
    });
    const job = result.current.run(async () => log.push("rename"));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(log).toEqual([]);

    release();
    await act(() => Promise.all([flushed, job]));
    expect(log).toEqual(["save slow", "rename"]);
  });

  it("discard drops pending changes and ignores later ones", async () => {
    const save = vi.fn(async (_: string) => {});
    const { result, unmount } = renderHook(() => useAutosave(save, 500));
    act(() => result.current.schedule("doomed"));
    act(() => result.current.discard());
    act(() => result.current.schedule("ignored"));
    unmount();
    await vi.runAllTimersAsync();
    expect(save).not.toHaveBeenCalled();
  });
});
