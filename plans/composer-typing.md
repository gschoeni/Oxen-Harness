# Composer typing responsiveness — 2026-09-18

## Diagnosis and change

`Composer` resized its textarea inside every `onChange`: write `height: auto`,
read `scrollHeight` (forcing layout), then write the measured height. The chat
and empty-state hero share that flex layout. All three toolbar pickers also
rendered on every draft change, despite not using the draft. The hero's game
loop is already idle before play and ignores keys aimed at inputs.

Use CSS `field-sizing: content` when supported. Older webviews use
`useComposerSize`, which coalesces measurements into animation frames and
observes width changes (window/dock resizing), ignoring height-only changes.
Value changes include send, clear, and slash-command selection. Keep the
200px cap and scrolling for longer drafts. Memoize the toolbar as one boundary;
its store subscriptions and busy prop still update independently.

## Verification

- Regression tests cover immediate draft updates without synchronous layout
  reads or toolbar renders, batched fallback sizing, shrink after send,
  cancellation on unmount, native sizing, width changes without observer loops,
  session labels, focus, and sending while busy. Existing chat tests cover
  slash commands and attachments.
- TypeScript and all 591 frontend tests passed (64 files).
- Real Chrome and Playwright WebKit checks rendered the composer with the
  empty-state hero. A 47-character prompt made 47 `scrollHeight` reads before
  the change, zero with native sizing, and 20–21 batched fallback reads.
  These are operation counts, not an end-to-end latency benchmark.
- Both browsers passed multiline/Shift+Enter, the 200px cap and overflow,
  clearing after send, and width-change wrapping. At 1000px → 430px viewport
  width the old draft stayed 103px high; both new paths grew it to 200px.
- Workspace Clippy passed. Required wider checks exposed unrelated failures:
  workspace fmt reports existing Rust formatting differences; bridge Clippy
  flags the redundant closure at `commands/files.rs:834`; `cargo test` has
  ten CLI composer/completion failures with shared draft state. Nextest
  isolates those CLI tests, but two preview crash tests timed out before their
  Python servers began serving (915 passed before fail-fast).

## Review

Reviewed the feature diff after commit `586ed15` for modularity, maintainability,
readability, idiomatic React, and pragmatism:

1. Keep measurement lifecycle in its small hook and use a single toolbar memo
   boundary. No need to memoize the full chat or move the draft into the store.
2. Consolidate the two animation-frame test doubles. One used a queue while the
   other retained an already-fired callback, which could mask scheduling bugs.
   Applied a shared queue with cancellation and one-shot flush semantics.
3. Keep the native path free of JS measurements and avoid a hidden measuring
   textarea or a new autosizing dependency. No additional production refactor
   was warranted.

Post-review verification:

- TypeScript and all 591 frontend tests passed again.
- Workspace Clippy passed. Nextest with four test threads and fail-fast
  disabled passed all 1,332 tests (5 skipped), including the preview crash
  tests that failed under the initial heavier parallel load.
- Workspace fmt and bridge Clippy still report the unrelated issues above;
  no Rust files were edited for this fix.
- `scripts/gc-target.py` completed after testing, reclaiming 1.6 GB on its first
  pass. Browser profiling fixtures were removed after use.

## Follow-up: first-use native input (2026-09-18)

The user still feels a first-use hitch after the layout fix. Profiling the full
React app with synthetic IPC data in Chrome and Playwright WebKit showed typing
updates around 0–1 ms, without reproducing a long rendering stall. This fixture
does not reproduce the user's real history, native text services, or OS keyboard
input, so it cannot rule out a desktop-only stall.

Both the chat and project-start prompt left native text services enabled.
[WebKit documents](https://webkit.org/blog/15865/webkit-features-in-safari-18-0/)
that inline predictions default to enabled, and can be disabled per element
using `writingsuggestions="false"`. Both prompts now disable these suggestions,
spellchecking, autocorrection, and automatic capitalization. This intentionally
removes native spelling underlines/corrections from prompt fields; it is a
targeted mitigation, not proof that those services caused the reported hitch.

The same input audit found Enter intercepted during native composition. Both
prompts now leave composing keys to the IME, including WebKit's keyCode 229
commit event, then send normally on a subsequent Enter. Four regression cases
failed before the guards and pass after them.

Verification before the follow-up review:

- Chrome and WebKit verified all four native-input attributes on both prompt
  surfaces, retained composing text, and rendered the full app successfully.
- TypeScript passed. Frontend tests: 612 passed, one unrelated MediaPage budget
  save test failed. All 1,338 Rust tests passed (5 skipped).
- Workspace fmt and bridge Clippy retain their existing failures. The first
  workspace Clippy/TypeScript attempts also caught temporarily inconsistent
  protocol/tab changes being edited elsewhere in this shared worktree; no such
  files were changed for this task.
- The user's running app was a debug build whose original Vite server stopped
  during investigation; restarting that development instance is required to
  load the edits. No running user app or draft was deliberately restarted.

Follow-up review after `60d7bb9`:

1. Consolidate the native input attributes and composition guard so chat and
   project prompts cannot drift. Applied the small `lib/promptInput.ts` module.
   Keep its types native to the DOM; no React declaration merging is needed.
2. Keep the controlled textarea and existing sizing. The measured React work
   does not justify switching to an uncontrolled editor or adding scheduling.
3. Defer project-page render isolation: it is a separate potential hot path,
   but this run has not established that it contributes to the reported hitch.

Post-review checks: TypeScript and all 613 frontend tests passed; all 1,338 Rust
tests passed (5 skipped); workspace Clippy passed. Workspace fmt and bridge
Clippy still have the previously recorded unrelated failures. Chrome and WebKit
passed the native-input and composition checks on both prompts again. Build
cache garbage collection completed after the first test pass (3.9 GB reclaimed)
and was run again after final checks.

The profiling server also logged full-app HMR invalidations from unrelated
edits to shared UI modules during this run. These are another possible source
of pauses in a development build; the trace does not establish that a reload
coincided with the user's originally reported keystrokes.
