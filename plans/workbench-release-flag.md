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
failure. Full checks and the dedicated review are recorded below when complete.

Feature verification passed: workspace formatting, workspace Clippy with warnings
as errors, 1,399 Rust tests (5 skipped), native Clippy, TypeScript and all 675
frontend tests. Production frontend build also passed (existing chunk-size
advisory only).
