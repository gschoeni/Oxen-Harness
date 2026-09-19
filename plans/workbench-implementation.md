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

### Feature commit and dedicated review

Feature committed as `e385691` using a temporary index and three-way patches
against the initial source snapshot. Existing staged renames and unrelated work
were preserved. The committed changes add the workflow, bundled/installed
module paths, shared document service, transports, and examples.

Review findings to address in the separate polish commit:

1. The graph component grew too large; separate reusable node/parameter/output
   rendering and lazy-load it so normal chats do not load React Flow.
2. An aborted validation future can retain a run reservation. A rebuilt session
   can lose sight of its live runs. Give reservations RAII cleanup and share run
   lifecycle state across session-engine rebuilds.
3. A late document reload can replace a just-saved snapshot. Fence reloads with
   document generations and test save/load overlap. Closing a view should retain
   drafts without a misleading discard confirmation.
4. Strengthen package lifecycle: revoke/close instances on removal/update, deny
   new windows, verify copied assets against their approved hash, and preserve
   the package's own view id when the graph creates a file.
5. Verify all model reference fields are supplied via typed graph inputs, honor
   the session's rewrite-token budget, and preserve contextual errors.
6. Match the remaining source/state adapters to their conversation context;
   avoid a global Gallery focus leaking across top-level tabs.

Meaningful verification: Rust Nextest 1,346 pass / 5 skipped; frontend 618 pass;
workspace and native Clippy pass; tsc pass; standalone workflow package build
pass. Browser check: five initial nodes, zero right-panel tabs, add/edit/save a
sixth node, no page errors. No paid operations were used. `cargo test --workspace`
exposed existing CLI tests sharing process-global composer state; Nextest's
process isolation (the project's preferred runner) passes them all.

### Review completed

Addressed the six findings above. The graph now loads lazily and separates card
and inspector rendering. Run reservations release on cancelled validation;
rebuilding a session retains active runs and their permission gate. Reloads are
fenced against newer saves. Package removal/update closes instances, new windows
are denied, and installed assets are checked against the approved hash. Package
runs preserve stale-revision rejection while pinning the validated graph.
Gallery selections and late SDK composer callbacks retain their owning chat.
The text editor keeps its read-only preview for oversized files.

Two additional review fixes: contributor modules advertise patterns/schema to
the agent without a Rust registry edit; the generic picker only carries compatible
resources into its next view. Regression tests cover descriptor discovery/open,
package identity, late callbacks, concurrent drafts, path grants, asset tampering,
and reservation cleanup. Unknown future node kinds remain editable but cannot run.

Final checks (the shared checkout, including its pre-existing work):
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo nextest run`: 1,350 passed, 5 skipped.
- Desktop `cargo clippy -- -D warnings`: passed.
- Desktop command-isolation unit test: passed.
- `pnpm exec tsc --noEmit`: passed.
- `pnpm exec vitest run`: 622 passed across 74 files.
- `pnpm build` and `pnpm build:workflow-view`: passed.
- Chrome: five starter nodes, zero panel tabs; click/edit/save node six, drag/drop
  node seven, move an existing node, save and verify positions; no page errors.
  Temporary browser fixtures were removed. Screenshots/logs remain in `/tmp`.
- No live paid requests. Native installed-view interaction and Windows/Linux
  rendering still need a manual platform smoke test.

Verification compatibility fix `944064e` boxes the provider failure payload and
removes a redundant native closure. Existing unrelated source edits, staged
renames, and accompanying style-ratchet repairs remain outside feature commits.
The polish commit is staged from the post-feature snapshot using a temporary
index so unrelated ongoing work stays intact.

Post-suite `scripts/gc-target.py` completed: removed 1.2 GB of rebuildable artifacts.
