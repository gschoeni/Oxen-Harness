// Trailing-edge throttle for furiously-updating values (streaming tool args,
// a canvas document forming). The store updates on every batched event; views
// that are expensive to render (syntax highlighting, markdown) subscribe
// through this so they re-render at a human cadence instead of an event one.

import { useEffect, useRef, useState } from "react";

/** The latest `value`, re-emitted at most once per `ms`. Trailing-edge: the
 *  final value always lands within `ms` of its arrival. */
export function useThrottled<T>(value: T, ms: number): T {
  const [shown, setShown] = useState(value);
  const latest = useRef(value);
  latest.current = value;
  const timer = useRef<number | null>(null);

  // Runs after every render: schedule one trailing emit when the shown value
  // is behind, and let an in-flight timer absorb any further updates.
  useEffect(() => {
    if (timer.current != null || Object.is(latest.current, shown)) return;
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setShown(latest.current);
    }, ms);
  });

  // The pending emit dies with the component, not with the next update.
  useEffect(
    () => () => {
      if (timer.current != null) window.clearTimeout(timer.current);
    },
    [],
  );

  return shown;
}
