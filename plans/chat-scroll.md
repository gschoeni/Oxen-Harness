# Natural chat scrolling

## Diagnosis

The chat styled all programmatic scrolling as smooth, then interpreted every
intermediate scroll event more than 80px from the bottom as a reader scrolling
up. A Chromium reproduction of the previous component left the viewport 1,100px
behind the output and displayed the arrow without any user input.

Following also depended only on message-array updates. Independently rendered
file previews, loaded images, and viewport changes from panels or the composer
could leave the view behind. Switching sessions reset the follow flag after the
layout effect, so switching from a paused chat could miss the initial scroll.

## Behavior

- Streaming follows immediately; no competing smooth-scroll animations.
- Upward scrolling pauses following, including small movements. Wheel intent is
  recorded before a concurrent content update; nested code scrolling is separate.
- Both content and viewport resizing preserve following, or leave the reader's
  position alone when paused. Browser scroll anchoring remains enabled.
- Reaching the bottom, clicking the existing arrow, submitting a prompt, and
  selecting another chat resume following.
- The observer disconnects on unmount. No polling or permanent animation loop.

`useChatScroll.ts` owns this policy; Chat owns the markup and existing arrow.
Eleven integration regressions cover scrolling, resizing, resumption, session
switching, nested scrolling, and cleanup. Six failed against the old component.
A pre-existing agent test fixture also used an invalid fleet source; it now uses
`turn` so TypeScript can check the frontend.

## Feature verification

- Current workspace: TypeScript and all 477 frontend tests passed; Tauri Clippy
  passed. Chromium passed 13 checks, including rapid streaming, delayed growth,
  keyboard scrolling, resizing, session changes, and deferred layout in a
  100-message thread. No browser errors. Screenshot: `/tmp/oxen-scroll.png`.
- Isolated HEAD plus this change: TypeScript and all 396 frontend tests passed.
- Workspace Rust formatting is blocked by the unrelated unfinished
  `harness-media` crate, which declares a missing `src/tools.rs`. Isolated HEAD
  also has existing formatting differences in CLI files and harness-store.
- Isolated workspace Clippy, nextest, and Tauri Clippy cannot compile the
  pre-existing `Agent::invalidate_tool_cache` call: that method is absent from
  committed HEAD. These backend failures are independent of the frontend diff.
  The working tree contains the method, explaining why its Tauri check passes.
