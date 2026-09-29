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

## Dedicated review

Reviewed feature commit `b677121` for state ownership, renderer/host lifecycle,
readability, and practical regressions. Apply these targeted improvements:

- Opening work in a narrow window must consider the file column's default width,
  not treat an unsaved width as zero. Run the same reveal/fit logic for legacy
  `open_file`, canvas and preview events even when the new panel was hidden,
  rather than only when it was collapsed. Cover actual App layout at 800 px,
  and retain the file column at 1,024 px when both panels can fit.
- App-scoped package loading can now begin before a conversation exists. Wait
  for the first session, load once per app mount, and route registration/load
  failures back to their initiating conversation if the user switches tabs.
- Update the old App comment that implied preview status hydration reveals the
  panel. Leave history and manual collapse behavior intact.

Final checks after review passed: workspace formatting, workspace/native Clippy,
1,399 Rust tests (5 skipped), TypeScript, all 691 frontend tests and the production
build. Narrow-window and discovery-lifecycle regressions were observed failing
before their fixes. The build retains its existing chunk-size advisory.
Build-cache garbage collection completed; no stale artifacts needed removal.
