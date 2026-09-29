# Work panel release scope — 2026-09-28

Keep file viewing available while holding customization for a later release.
One host-owned startup flag, `OXEN_WORKBENCH_CUSTOMIZATION=1` (or `true`), enables
the experimental experience; unset, false, empty or unrecognized values keep it
disabled. No user settings toggle or separate frontend build flag.

Scope:

- Hide the view picker and welcome cards when off; show the current file/view
  name and a simple file-focused welcome state.
- Exclude View Studio, Manage views and installed packages from UI discovery.
- Preserve normal file-type resolution, preview/canvas/browser/gallery surfaces,
  saved workflow graphs, conversation tabs, pinning and work history.
- Block authoring and native package operations; omit `develop_view` from the
  agent's tools. Ordinary bundled renderer descriptors remain supported.
- Keep old files/packages/history intact. Stale authoring targets return to the
  welcome screen; package files fall back to a built-in viewer.
- Load the host's flags before rendering; a failed load leaves customization
  disabled and reports the failure through the app's notice surface.

## Verification

Regression coverage exercises both flag states, UI discovery, stale history,
file resolution, direct host calls, authoring tool availability and startup
failure. Full checks and the dedicated review are recorded below.

Feature verification passed: workspace formatting, workspace Clippy with warnings
as errors, 1,399 Rust tests (5 skipped), native Clippy, TypeScript and all 675
frontend tests. Production frontend build also passed (existing chunk-size
advisory only).

## Dedicated review

Reviewed feature commit `7bdac97` for modularity, readability, frontend/native
agreement, default-off behavior and preserved file navigation. Concrete changes:

- Normalize disabled targets at the store's navigation boundary as well as the
  rendering boundary. Otherwise travelling back to a saved package file can
  show the editor's previously active file under the restored filename. Add a
  regression using real work history and editor state before fixing it.
  Preserve the existing context when hydrating its fallback file on startup,
  so loading that file does not replace saved forward history.
- Memoize the effective display target so unrelated parent renders do not
  recreate module APIs and restart subscriptions for a disabled history entry.
- Exercise opt-in authoring tool availability and installed-package discovery
  across enabled/disabled hosts, not just scaffolding and hidden UI controls.
- Format the conditional selector/welcome JSX for readability. Keep the switch
  startup-only; adding settings, runtime flag subscriptions or a generic flag
  framework would be unnecessary for this release.
- Defer a startup failure notice until the first session is restored. The chat
  notice store deliberately drops messages without a session, so reporting
  before mounting App would hide the failure.

After review, workspace/native formatting and Clippy, 1,399 Rust tests (5
skipped), TypeScript, all 678 frontend tests and the production build passed.
The two history regressions were observed failing before their fixes.
Build-cache garbage collection completed successfully and freed 8.5 GB.
