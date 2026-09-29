# Work panel opens on demand — 2026-09-29

The panel must start hidden and open when a user opens a file or an agent tool
explicitly presents work. An empty chat should not reserve a right column or
show a collapsed rail. Preserve file-type rendering and saved work history.

Implementation:

- `rightTab` records views explicitly opened during this app run, rather than
  being initialized from persisted contexts. Work contexts/history remain on disk.
- The right dock requires a request for the active conversation and an available
  target; welcome/disabled customization targets do not open an empty surface.
- A visible conversation's `open_view` expands the panel, including repeated
  opens of the same file. Background requests retain their own conversation.
- Move registration/package discovery to an app-scoped hook so an agent can
  discover a view before the panel has ever mounted.

Regression tests cover empty/startup layouts, saved history, file clicks,
agent requests, collapsed reopen, background conversations, passive updates,
disabled customization and module discovery while hidden.

## Verification and review

Feature checks passed: workspace formatting, workspace/native Clippy, 1,399 Rust
tests (5 skipped), TypeScript and all 684 frontend tests. Dedicated review follows.
