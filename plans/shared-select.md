# Shared view selector — 2026-09-19

The work-view picker used an operating-system select with titles alone. Reuse the
existing `Menu` and `MenuItem` components, add the standard `Select` composition,
and show module-owned icons and descriptions without introducing another tab bar.

Feature commit: `b2b0a29`.

The control supports disabled options, selected checks, a labelled combobox and
listbox, described options, arrows/Home/End/typeahead, Enter/Space selection,
Escape cancellation, Tab navigation, outside dismissal and a viewport-aware
portal. The shared `.menu` surface participates in native-preview overlay hiding.
Icons remain local bundled-module metadata; installed packages receive a fallback.
View changes keep only compatible resources. The README establishes `Select` as
the default for new single-choice selectors; custom/search pickers retain `Menu`.

## Dedicated review

Reviewed the committed feature for component ownership, focus/keyboard behavior,
module extensibility, theme tokens, clipping, and regressions in existing menus.
Applied these concrete improvements:

- If the active option disappears or becomes disabled while open, derive a valid
  fallback immediately. A regression test demonstrates the stale ARIA reference
  before the fix and keyboard selection of the replacement after it.
- Anchor upward menus by their bottom edge and observe content size changes, so
  wrapping/descriptions and viewport changes cannot detach the menu from its
  trigger. Avoid redundant positioning updates.
- Keep description/copy styling with the shared menu primitives instead of the
  selector stylesheet, so any menu can use the new description capability.

Feature verification passed TypeScript, 632 frontend tests and the production
build. Browser verification exercised light/dark themes, a 340-pixel window,
clipped parent containers, keyboard selection, Tab focus and upward placement,
with no page errors. The same browser check passed after review. The initial
Rust checks and cache cleanup encountered an unrelated in-progress `CallContext`
refactor; no Rust files were changed for this feature.

Final verification: TypeScript, **633 frontend tests / 78 files**, production
build, browser checks, formatting and native Clippy passed. Nextest passed
**1,368 tests / 5 skipped**, with one unrelated pre-existing fallback-usage test
reported as leaky. Workspace Clippy with warnings denied remains blocked by
`clippy::too_many_arguments` in `crates/harness-agent/src/agent/tools.rs:349`
(`run_repaired`, 8 arguments) from the concurrent `CallContext` refactor. That
work is preserved and excluded from these commits.

Post-suite cache cleanup completed, reclaiming 2.5 GB of rebuildable artifacts.
