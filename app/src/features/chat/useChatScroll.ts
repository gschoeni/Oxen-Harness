import { useCallback, useLayoutEffect, useRef, useState, type WheelEvent } from "react";

/** Within a pixel or two of the end counts as the bottom: WebKit reports
 *  fractional scroll offsets, so an exact comparison flickers. */
const atBottom = (el: HTMLElement) => el.scrollHeight - el.clientHeight - el.scrollTop <= 2;

/** Keep the chat pinned to its live tail until the reader scrolls up, and
 *  resume the moment they return to the bottom (by hand or via the arrow).
 *
 *  Two signals mean "the reader moved up": a scroll event whose offset is
 *  smaller than the last one we saw, and an upward wheel gesture. The wheel
 *  matters on its own because a fast stream can pin the tail again before the
 *  gesture's scroll event arrives, which would otherwise swallow the intent.
 *  Nothing else can lower the offset: our pins only move down, and the
 *  browser clamps a shrinking thread to the bottom, where we re-follow. */
export function useChatScroll(sessionId: string | undefined, items: unknown) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const lastTop = useRef(0);
  const [paused, setPaused] = useState(false);

  const follow = useCallback((value: boolean) => {
    following.current = value;
    setPaused(!value);
  }, []);

  const pin = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    lastTop.current = el.scrollTop;
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

  // Both matter: images/code can grow without a new message, and panels or
  // the composer can shrink the viewport without changing the thread.
  useLayoutEffect(() => {
    const viewport = scrollRef.current;
    const content = contentRef.current;
    if (!viewport || !content) return;
    const observer = new ResizeObserver(() => {
      if (following.current) pin();
      else if (atBottom(viewport)) follow(true);
      lastTop.current = viewport.scrollTop;
    });
    observer.observe(viewport);
    observer.observe(content);
    return () => observer.disconnect();
  }, [follow, pin]);

  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    if (atBottom(el)) follow(true);
    else if (el.scrollTop < lastTop.current) follow(false);
    lastTop.current = el.scrollTop;
  }

  function onWheel(event: WheelEvent<HTMLDivElement>) {
    const el = scrollRef.current;
    if (!el || event.deltaY >= 0 || el.scrollTop <= 0) return;
    // A code block's own scrolling must not detach the surrounding chat.
    for (let target = event.target instanceof Element ? event.target : null;
      target && target !== el; target = target.parentElement) {
      if (target.scrollTop > 0 && /auto|scroll/.test(getComputedStyle(target).overflowY)) return;
    }
    follow(false);
  }

  return { scrollRef, contentRef, paused, scrollToBottom, onScroll, onWheel };
}
