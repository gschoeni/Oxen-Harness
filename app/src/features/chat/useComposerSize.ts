import { useEffect, useRef, type RefObject } from "react";

/** Older webviews need a measured height; keep that work out of input events. */
export function useComposerSize(ref: RefObject<HTMLTextAreaElement | null>, value: string) {
  const schedule = useRef<(() => void) | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el || globalThis.CSS?.supports?.("field-sizing", "content")) return;
    let frame: number | null = null;
    const resize = () => {
      if (frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        el.style.height = "auto";
        el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
      });
    };
    schedule.current = resize;
    // Dock/window changes can wrap text without changing its value. Ignore
    // height changes so our own resize doesn't start an observer loop.
    let width: number | undefined;
    const observer = new ResizeObserver(([entry]) => {
      if (entry && entry.contentRect.width !== width) {
        width = entry.contentRect.width;
        resize();
      }
    });
    observer.observe(el);
    return () => {
      observer.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
      schedule.current = null;
    };
  }, [ref]);

  useEffect(() => {
    schedule.current?.();
  }, [value]);
}
