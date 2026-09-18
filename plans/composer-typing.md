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

Pending the dedicated post-commit review and verification pass.
