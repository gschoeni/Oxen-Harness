# Workbench implementation record

Started 2026-09-18. Implements [the approved view plan](workbench-views.md).

## Product constraints

- The existing top conversation tabs own both agent and right-hand context.
  No independent dock, Editor, or Canvas tab strips.
- Shared files are authoritative. Unsaved work survives view switches; stale
  writes report a conflict. Background sessions never take over the active tab.
- Bundled modules first, then installable packages using the same contract.
- The first custom module is an Oxen node workflow with image generation,
  video generation, image/video upscaling, and LLM prompt rewriting. Graphs
  support drag/drop construction and agent edits to the same JSON files.
- Complete the graph reference, package host, contributor examples, verification,
  and a dedicated review/refactor pass before calling the feature done.

## Progress

- [x] Read current plan, architecture, conventions, and actual migration seams.
- [x] Record initial working-tree state and baseline checks.
- [ ] Shared document and view contracts, revision-aware persistence.
- [ ] One work context per conversation; common host and view navigation.
- [ ] Built-in adapters, durable canvas, agent list/open/inspect, transport parity.
- [ ] Editable graph, explicit Oxen-backed runs, persistent outputs and recovery.
- [ ] Scoped installable packages, public SDK, lifecycle and examples.
- [ ] Workflow presets, contributor documentation, native/browser inspection.
- [ ] Full verification, feature commits, review/refactor and re-verification.

## Baseline

The checkout contains substantial pre-existing staged/unstaged edits. A source
snapshot and index/working patches were saved outside the repository at
`/var/folders/ht/0ypwcxw55m37xf3l08vrzx6h0000gn/T/oxen-workbench-baseline-5g_2jgc7`.
Preserve those edits and stage only this feature's changes.

- `cargo fmt --all -- --check`: passed.
- `npx tsc --noEmit`: passed.
- `npx vitest run`: 606 passed, 2 failed, both existing sizing-ratchet failures
  (`styles/tokens.test.ts`) across ongoing UI edits and a removed ledger file.
- Workspace Clippy: existing `result_large_err` failures from the current
  `AgentError`, propagated into review/loop and subagent tests.

Initial document iteration: tests first failed on missing implementation, then
all four passed (revision conflict/recovery, independent writers, limits/path
validation, and symlink rejection). Shared path locks now coordinate agent
file writes and document saves across sessions.

The live unauthenticated Oxen model catalog was checked on 2026-09-18. It
advertises image and video upscalers, including `flux-image-upscaler` and
`flux-video-upscaler`, with actual parameter schemas. Workflow execution will
use those schemas and the existing media queue rather than invent endpoints.

## Implementation decisions and review

Record concrete decisions, check output, migration notes, and review findings
here as each iteration completes. Proposed package APIs in the original plan
can be refined against the working reference modules.

### Working implementation checkpoint

- Added runtime `documents`, `workflow`, `workflow_run`, `views`, `view_packages`.
- Added process-wide file-path locks used by UI saves and agent file edits.
- Added host `workbench`: transport-neutral document/view/run APIs, list/open/
  inspect/run agent tools, per-session permission gate, Oxen media generation
  and bounded prompt rewrites, durable graph snapshots/results/latest pointers.
- Added `view.open` protocol event, desktop command and HTTP workbench route.
- Added frontend `workbench-sdk`, module auto-discovery, per-conversation work
  contexts with history/pinning, one right Workbench column, view picker.
- Added bundled Oxen graph editor (`@xyflow/react`) with palette, typed edges,
  inspector/model schema fields, starters, explicit runs/cancellation/results.
- Text editor and graph now share revision-aware drafts; removed Editor and
  Canvas tab strips in favor of selects. Canvas now writes `.canvas.json`.
- Added immutable reviewed local packages, permission globs, native package
  surfaces and narrow IPC bridge. Main core capability now scopes webview
  `main`, not every child of its window. Native host commands reject child
  views except the instance-scoped `view_bridge` command.
- Added package manager UI; SDK `browser.js` is injected into package webviews.

Validation so far: document 4, workflow format 4, runner 2, packages 2 Rust
unit tests passed; 5 new frontend tests passed. Frontend typecheck passed
before package UI (one React 19 useRef signature fixed since). Full frontend
611 pass, 2 known baseline token failures. Host + HTTP check passed. Latest
native check compiled feature but Clippy found an existing redundant closure
in `commands/files.rs:834`. Backend test run briefly hit missing ThreadRow
exports during concurrent edits; those exports exist again, rerun needed.

Still required: finish package integration/native validation, example packages
and contributor docs, stronger host/graph integration tests, feature bug review,
full check suite and GC, selective feature commits, dedicated polish commit.
No paid Oxen generations have been run.
