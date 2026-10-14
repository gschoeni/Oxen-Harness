// Horizontal pointer drag against an element's bounds — the timeline scrub.

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

export interface UsePointerDragArgs {
  /** Element whose horizontal bounds map the pointer to a 0–1 fraction. */
  targetRef: RefObject<HTMLElement | null>;
  /** Called with the clamped fraction on drag start and every move. */
  onFraction: (fraction: number) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
}

/** The rect is measured once per drag (reading layout per pointermove would
 *  force a reflow between the drag's own style writes) and re-measured if the
 *  element resizes mid-drag — e.g. the player entering theater mode. */
export function usePointerDrag({ targetRef, onFraction, onDragStart, onDragEnd }: UsePointerDragArgs) {
  const [isDragging, setIsDragging] = useState(false);
  const rectRef = useRef<DOMRect | null>(null);
  const pointerIdRef = useRef<number | null>(null);
  const callbacksRef = useRef({ onFraction, onDragStart, onDragEnd });
  useEffect(() => {
    callbacksRef.current = { onFraction, onDragStart, onDragEnd };
  });

  const applyClientX = useCallback(
    (clientX: number) => {
      const rect = (rectRef.current ??= targetRef.current?.getBoundingClientRect() ?? null);
      if (!rect || rect.width <= 0) return;
      callbacksRef.current.onFraction(Math.min(1, Math.max(0, (clientX - rect.left) / rect.width)));
    },
    [targetRef],
  );

  useEffect(() => {
    if (!isDragging) return;
    // Listening on window: a second pointer must not steer or end the drag.
    const move = (event: PointerEvent) => {
      if (event.pointerId !== pointerIdRef.current) return;
      event.preventDefault();
      applyClientX(event.clientX);
    };
    const stop = (event: PointerEvent) => {
      if (event.pointerId !== pointerIdRef.current) return;
      setIsDragging(false);
      callbacksRef.current.onDragEnd?.();
    };
    const invalidate = () => {
      rectRef.current = null;
    };
    const observer = typeof ResizeObserver !== "undefined" ? new ResizeObserver(invalidate) : null;
    if (observer && targetRef.current) observer.observe(targetRef.current);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
    window.addEventListener("resize", invalidate);
    return () => {
      observer?.disconnect();
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      window.removeEventListener("resize", invalidate);
    };
  }, [isDragging, applyClientX, targetRef]);

  /** True when a drag began. Only the primary button starts one: a
   *  right-click's context menu swallows the pointerup and would wedge it. */
  const beginDrag = useCallback(
    (event: { clientX: number; button: number; isPrimary: boolean; pointerId: number }) => {
      if (event.button !== 0 || !event.isPrimary) return false;
      pointerIdRef.current = event.pointerId;
      rectRef.current = targetRef.current?.getBoundingClientRect() ?? null;
      callbacksRef.current.onDragStart?.();
      setIsDragging(true);
      applyClientX(event.clientX);
      return true;
    },
    [applyClientX, targetRef],
  );

  return { isDragging, beginDrag };
}
