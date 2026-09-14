# Error handling audit — 2026-09-13

Reviewed Rust production `unwrap`, `expect`, panic macros, and platform `unsafe`
blocks across the workspace and desktop bridge. Test assertions and deliberate
failure injection are distinct from runtime error handling.

## Changes

- History locking is fallible throughout the store. Poisoned connections return
  an error advising a restart, without continuing through inconsistent state.
- Filesystem probes preserve syscall errors and identify the database path.
  Failed probes cannot silently enable SQLite WAL on an unknown mount. Platforms
  without a probe retain rollback journaling.
- Chat request serialization returns errors through both HTTP call paths.
- Shell state locks and state-file reads report useful tool errors. Invalid
  process-group IDs are rejected, and failed kill syscalls no longer report a
  successful stop or mark the task killed.
- Preview config serialization is fallible. Saving refuses to overwrite corrupt
  config, and a running server's tool result explains when its settings were not
  saved.
- Git output capture reports pipe, read, reader-thread, wait, and cleanup errors
  instead of returning empty successful output. Screenshot callbacks check native
  pointers and use a main-thread `Cell` for the completion sender. Desktop startup
  and file-watch commands return errors instead of panicking.
- Queue draining, markdown table rows, tool waves, argument repair, fallback
  selection, worktree restoration, review results, and schema export no longer
  use panic-based assumptions at the reviewed sites.
- `AGENTS.md` now requires useful propagated errors, prohibits production
  unwrap/expect, forbids hiding operational failures with defaults, and requires
  checked, documented platform FFI.

## Remaining audit findings

This is not a claim that every production panic source has been removed. The
remaining widespread issue is infallible shared-state APIs whose locks still
use `expect`/`unwrap`:

- Permissions and grants (`harness-permissions/src/lib.rs`). These must fail
  closed and report an error, never silently recover or default to permission.
- Fleet spawners, budgets, lane registries, fork state, and interjections
  (`harness-agent`); host pending requests and streaming buffers (`harness-host`).
- File snapshots, path locks, tool rosters, asides, and foreground tracking
  (`harness-tools`); overflow storage (`harness-compress`).
- Preview lifecycle/manager/console state (`harness-preview`), desktop browser
  and preview state, and CLI caches, composer, and fleet painting.
- In-progress media library/registry state. Its files were already untracked or
  modified on entry; its two local optional-value fixes stay with that work.

Migrating these APIs requires carrying errors through their callers and
callbacks, with failure injection at the permission/budget/edit boundaries.
Replacing every lock with a non-poisoning lock or `into_inner()` would hide
possibly inconsistent state and does not satisfy this audit's rule.

Unavoidable native FFI remains for filesystem capacity, process groups, crash
signals, and WebKit screenshots. Follow-up work remains for error reporting in
preview process-group cleanup and crash-handler installation. Signal-time
marker writes are necessarily best effort; they cannot allocate, lock, or use
the ordinary UI reporting path.

## Verification and review

- The poisoned-history regression failed with a second panic before the fix.
- The corrupt-preview-config regression failed because saving returned success
  and overwrote the file before the fix.
- Regression coverage checks history errors, refusal to execute with poisoned
  shell state, invalid process IDs, filesystem probe failures, config
  preservation, and the warning returned alongside a successfully started server.
- Working-tree workspace fmt, Clippy, nextest, desktop Clippy, TypeScript and
  Vitest are recorded below after final verification.
- An isolated commit checkout excludes existing media/attachment work. Its
  baseline already fails compilation: `Agent::make_tool_less` calls missing
  `invalidate_tool_cache`. The working tree supplies that existing implementation.
  Commit-only isolation therefore cannot independently establish a green build.
- Dedicated review/refactor findings and final check totals follow in the polish
  commit.
