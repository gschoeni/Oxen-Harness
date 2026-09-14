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


## Dedicated review / polish

Reviewed the committed diff for modularity, readability, and pragmatic behavior:

1. **Fix stale geometry when resuming after a resize.** If collapsed content
   brings a paused reader to the bottom, the next upward scroll must compare
   against the new dimensions. Added a regression (failed before the fix) and
   refreshed the saved position in the resize observer.
2. **Keep the scroll policy local.** The dedicated hook separates layout and
   intent from attachment/composer behavior without adding a generalized scroll
   framework or animation state machine. Expanded the directional condition for
   readability; no further abstraction was warranted.
3. **Check actual browser behavior separately from mocked layout.** The tests
   exercise React integration with controlled geometry; Chromium checks cover
   actual scrolling, delayed layout, and the existing content-visibility styles.

The review adds a twelfth integration regression. Backend verification is also
sensitive to ongoing unrelated edits: a later working-tree Clippy/nextest run
failed in `harness-media` and on a missing `harness_llm::base64_len` export. No
backend files are changed by this feature.

## Final verification

After polish, TypeScript and all 478 working-tree frontend tests passed; the
isolated frontend passed TypeScript and all 397 tests. Workspace formatting and
working-tree Tauri Clippy passed. Workspace Clippy and nextest remain blocked by
unrelated missing `harness_config::paths::cache_dir` and `harness_llm::base64_len`
symbols in the ongoing backend edits. Existing React act warnings remain in the
frontend suite. The scroll feature and review do not claim a green Rust suite.

Feature commit: `0155668`. Only this feature's frontend/docs changes and the
small fleet test-fixture correction were staged; other working-tree changes were
preserved.
