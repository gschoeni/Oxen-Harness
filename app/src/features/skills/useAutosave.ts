// Save-as-you-type for the skill editor: every change is written to disk a
// moment after the typing stops, or right away when focus leaves the field or
// the editor closes, so closing the window mid-thought never loses work.
//
// Writes run one at a time, in order, so an older save can never land after
// a newer one. Other operations on the same file (a rename, a delete) queue
// through `run` so they can't interleave with a save either.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

/** How long typing must pause before a save. */
export const AUTOSAVE_DELAY_MS = 600;

export type AutosaveStatus = "saved" | "pending" | "saving" | "error";

export interface Autosave<T> {
  status: AutosaveStatus;
  /** The last failed save's message, until a save succeeds. */
  error: string | null;
  /** Record a change; it is saved once typing pauses. */
  schedule: (value: T) => void;
  /** Save any unsaved change now. Rejects if that save fails. */
  flush: () => Promise<void>;
  /** Run `job` after every queued save (e.g. a rename of the saved file). */
  run: <R>(job: () => Promise<R>) => Promise<R>;
  /** Drop unsaved changes and stop saving (the file is being deleted). */
  discard: () => void;
}

export function useAutosave<T>(
  save: (value: T) => Promise<void>,
  delayMs = AUTOSAVE_DELAY_MS,
): Autosave<T> {
  const [status, setStatus] = useState<AutosaveStatus>("saved");
  const [error, setError] = useState<string | null>(null);

  const saveRef = useRef(save);
  saveRef.current = save;
  const latest = useRef<{ value: T } | null>(null); // unsaved change, if any
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const discarded = useRef(false);

  const clearTimer = () => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  };

  const run = useCallback(<R>(job: () => Promise<R>): Promise<R> => {
    const next = queue.current.then(job);
    queue.current = next.catch(() => {}); // a failure is reported once, by its caller
    return next;
  }, []);

  const flush = useCallback((): Promise<void> => {
    clearTimer();
    return run(async () => {
      const pending = latest.current;
      if (!pending || discarded.current) return;
      latest.current = null;
      setStatus("saving");
      try {
        await saveRef.current(pending.value);
      } catch (e) {
        // Keep the change so the next flush retries it, unless newer typing
        // already replaced it.
        latest.current ??= pending;
        setError(String(e));
        setStatus("error");
        throw e;
      }
      setError(null);
      setStatus(latest.current ? "pending" : "saved");
    });
  }, [run]);

  const schedule = useCallback(
    (value: T) => {
      if (discarded.current) return;
      latest.current = { value };
      setStatus("pending");
      clearTimer();
      // The failure is already on screen via `status`/`error`.
      timer.current = setTimeout(() => void flush().catch(() => {}), delayMs);
    },
    [flush, delayMs],
  );

  const discard = useCallback(() => {
    clearTimer();
    discarded.current = true;
    latest.current = null;
  }, []);

  // Leaving the editor any way at all (back, another settings page, closing
  // settings) still writes the last keystrokes.
  useEffect(() => () => void flush().catch(() => {}), [flush]);

  return useMemo(
    () => ({ status, error, schedule, flush, run, discard }),
    [status, error, schedule, flush, run, discard],
  );
}
