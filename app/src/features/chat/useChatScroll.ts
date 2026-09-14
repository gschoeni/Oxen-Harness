import { useCallback, useLayoutEffect, useRef, useState, type WheelEvent } from "react";

type Position = { top: number; height: number; viewport: number; width: number };
const position = (el: HTMLElement): Position => ({
  top: el.scrollTop,
  height: el.scrollHeight,
  viewport: el.clientHeight,
  width: el.clientWidth,
});
const atBottom = (p: Position) => p.height - p.viewport - p.top <= 1;

/** Follow output until the reader scrolls up; layout changes aren't user intent. */
export function useChatScroll(sessionId: string | undefined, items: unknown) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const previous = useRef<Position | null>(null);
  const [paused, setPaused] = useState(false);

  const follow = useCallback((value: boolean) => {
    following.current = value;
    setPaused(!value);
  }, []);

  const pin = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    // Instant movement avoids mistaking intermediate animation frames for a
    // user scroll, and lets rapid streaming updates reach the bottom each frame.
    el.scrollTop = el.scrollHeight;
    previous.current = position(el);
  }, []);

  const scrollToBottom = useCallback(() => {
    follow(true);
    pin();
  }, [follow, pin]);

  useLayoutEffect(() => {
    scrollToBottom();
  }, [sessionId, scrollToBottom]);

  useLayoutEffect(() => {
    if (following.current) pin();
  }, [items, pin]);

  useLayoutEffect(() => {
    const viewport = scrollRef.current;
    const content = contentRef.current;
    if (!viewport || !content) return;
    // Both matter: images/code can grow without a new message, and panels or
    // the composer can shrink the viewport without changing the thread.
    const observer = new ResizeObserver(() => {
      if (following.current) pin();
      else {
        const current = position(viewport);
        previous.current = current;
        if (atBottom(current)) follow(true);
      }
    });
    observer.observe(viewport);
    observer.observe(content);
    return () => observer.disconnect();
  }, [follow, pin]);

  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    const current = position(el);
    const last = previous.current;
    if (atBottom(current)) follow(true);
    else if (
      last && current.top < last.top && current.height === last.height &&
      current.viewport === last.viewport && current.width === last.width
    ) follow(false);
    previous.current = current;
  }

  function onWheel(event: WheelEvent<HTMLDivElement>) {
    const el = scrollRef.current;
    if (!el || event.deltaY >= 0 || el.scrollTop <= 0) return;
    // A code block's own scrolling must not detach the surrounding chat.
    for (let target = event.target instanceof Element ? event.target : null;
      target && target !== el; target = target.parentElement) {
      if (target.scrollTop > 0 && /auto|scroll/.test(getComputedStyle(target).overflowY)) return;
    }
    // Record intent before the next token/resize can arrive, including when
    // browser scroll anchoring changes the geometry during this gesture.
    follow(false);
  }

  return { scrollRef, contentRef, paused, scrollToBottom, onScroll, onWheel };
}
