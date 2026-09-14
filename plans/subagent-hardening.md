# Subagent hardening and minimal agent hub

The September 13 review found correctness gaps across isolation, cancellation,
terminal state, capabilities, budgets, and cached map results. This work fixes
those contracts before simplifying the CLI and desktop surfaces.

## Acceptance checklist

- [x] Capture committed and uncommitted lane changes against an immutable baseline;
      distinguish capture errors and preserve recoverable artifacts.
- [x] Fail closed on requested isolation; preserve lane workspace across nested
      delegation and follow-up, including durable patch access.
- [x] Per-lane stop reasons, cancellation during queueing and structured retries,
      terminal persistence before events, cleanup on every exit path.
- [x] Preserve disabled tools and overrides; use one agent-tool registration policy.
- [x] Bound/cancel tool-less batches with normal operation lifecycle and admission.
- [x] Explicit resumable map runs, refresh, and configuration-aware memo keys.
- [x] Stable CLI agent IDs, fleet selection, watch/send/stop/follow-up and artifacts.
- [x] Minimal desktop agent hub: nested/live/finished results, reliable controls,
      retained drafts, patch review/apply, clear partial and interrupted states.
- [x] Regression tests, all Rust/frontend/bridge checks, visual verification.
- [ ] Feature commits, then one documented review/refactor pass and polish commit.

## Working notes

- Existing user edits are preserved. A source baseline was saved outside the
  repository at `/tmp/oxen-harness-subagents-baseline` for selective commits.
- Four reproductions from the review are saved at
  `/tmp/oxen-harness-subagent-review-probe.rs`.
- UI direction: a single quiet Agents panel, compact status rows, details and
  actions on selection, restrained use of color, and existing theme tokens.

## Initial verification

1,258 Rust tests passed (3 skipped); fmt and workspace Clippy passed. TypeScript,
465 frontend tests, and Tauri bridge Clippy passed. The Agents panel was inspected
in Chromium in light/dark modes and at 390px width; no runtime errors or overflow.
