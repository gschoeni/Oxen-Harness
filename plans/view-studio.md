# Live view authoring

User intent: build app views with the agent, live preview/test/install them without
recompiling or restarting the desktop app. Preserve one conversation-owned work
surface and existing bundled/installed module contracts.

Implementation:
1. Shared runtime authoring service: atomic no-build scaffolds, immutable preview
   snapshots, unchanged-grant hot reload, last structurally valid revision,
   bounded diagnostics/test reports saved to project files, exact-revision install.
2. Session-owned host state, transport actions and a discoverable agent tool.
3. Native previews use the same scoped package surface as installed views.
   Injected SDK captures errors and runs registered tests in the actual preview.
4. View Studio: create/open, preview, reload, pause updates, test, inspect diagnostics,
   send context to agent, install. No separate tabs. Retain reports per source.
5. Portable browser SDK declarations and working starter with tests, accessible
   controls, theme tokens, draft retention, file conflict handling and AGENTS.md.
6. Regression tests, browser smoke tests, full required checks, cache GC, feature
   commit followed by a dedicated review/refactor and separate commit.

Constraints: source/workspace confinement and permissions still apply. A manifest
capability change pauses preview refresh until explicitly restarted. No remote
scripts, no desktop rebuild, no paid generations in verification. Build-based
frameworks can target a local output directory; do not require Node for the starter.

The checkout contains unrelated edits/staged renames. Baseline is recorded under
`/tmp/oxen-view-studio-baseline-path`; stage only this task using a temporary index.

## Feature verification

First full loop passed: fmt, workspace/native Clippy, nextest 1,354 passed / 5
skipped, frontend 624 passed / 75 files, tsc, desktop production build, standalone
workflow package build, native command-isolation test, and 3 portable SDK tests.
Browser smoke exercised three reloads, retained drafts, stale-save rejection after
an agent edit, composer context, 3 passing in-preview checks, deliberate test
failure, and console-error reporting; no page errors. Source/templates are tested
with the actual injected SDK and package content policy in Chrome; this does not
claim native WebKit/Windows/Linux interactive smoke coverage.

Dedicated review findings after feature commit `f06c49f`:
- Restore the source's previous report after host restart, marked inactive, without
  erasing diagnostics; distinguish reports for different sources in one chat.
- Block explicit restart while an unretained draft exists, and avoid overlapping
  browser test requests. Improve the initial invalid-package error surface.
- Count runtime exceptions in the browser smoke check; a body containing an error
  message is not evidence of a successful render. Bound diagnostics/test payloads.
- Add useful reload/rollback controls, accessible operation feedback, and stronger
  tests for lifecycle/state transitions. Review hot-reload cost and package cache.
- Separate native surface hosting and portable contracts from package-management
  UI; keep module code approachable and keep app settings/themes consistent.
- Re-run the full suite and browser check after review, GC build artifacts, and
  commit review changes separately. Existing unrelated edits must remain intact.

## Review outcome

Completed the review across the runtime service, host tool/transport, native
surface, portable SDK, starter, registry and Studio UI. Changes worth applying:

- Persist and restore source-specific reports as inactive; reject mismatched
  report ownership. Retain prior diagnostics without executing restored code.
- Give each preview a generation, even when its content hash is unchanged.
  Regression coverage proves status polling retains a native surface while a
  restart replaces it. Block restarts with an unretained draft.
- Add reload and previous-preview controls. Rollback pauses updates and leaves
  editable files untouched. Apply document-path changes explicitly, protecting
  drafts and avoiding remounts while typing.
- Load and verify a native package's complete asset map once per lease. Tests
  reject altered manifest metadata and prove later cache edits cannot change an
  already-mounted snapshot. Production replacement leaves development leases
  separate. Repeated status checks avoid re-parsing unchanged JavaScript.
- Separate native surface hosting from package-management UI. Keep one portable
  type contract and retain the small runtime/host/native boundaries.
- Runtime exceptions fail the smoke check. Bound diagnostics and UTF-8 test
  reports while preserving every reported pass/fail outcome. Show initial
  validation errors and accessible operation state.
- Fix a narrow-panel grid overflow found by the browser smoke check. Light and
  dark themes render without horizontal overflow at 340 pixels.

Final verification passed: `cargo fmt --all -- --check`, workspace and desktop
Clippy with warnings denied, nextest **1,357 passed / 5 skipped**, frontend
**626 passed / 76 files**, TypeScript, four portable SDK tests, native command
isolation, desktop production build and standalone workflow package build.
The Chrome package/SDK smoke passed again after review: three reloads, retained
drafts, preserved concurrent agent edits, registered tests, intentional failures
and captured diagnostics. Native interactive WebKit/Windows/Linux rendering and
paid generations were not exercised. Existing frontend test `act` warnings and
the main-app bundle-size advisory do not affect passing checks.

Post-suite `scripts/gc-target.py` completed and reclaimed 635.8 MB of rebuildable
artifacts. The review changes are committed separately from the feature; the
checkout's unrelated edits and three pre-existing staged renames are preserved.
