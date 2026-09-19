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

Dedicated review findings to address after the feature commit:
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
