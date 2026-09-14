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
- [x] Feature commits, then one documented review/refactor pass and polish commit.

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

## Dedicated review/refactor pass

Review of commit `40008e0` found these worthwhile changes:

1. Finalization removed a registration twice (explicitly and in Drop), leaving a
   race with a fast follow-up. Release exactly once; preserve capture failures as
   failures and include the recovery checkout path.
2. Memo loading published its ready flag before reading persisted rows; concurrent
   writers could persist older snapshots. Serialize cache initialization and writes.
3. The CLI's per-agent drafts lived only as long as one composer. Retain them in
   the fleet hub across turn/idle transitions and expose recovery for finished agents.
4. A queued display row could duplicate the same lane fetched from history before
   its start event. Reconcile unique fleet/name placeholders with persistent IDs.
5. User follow-ups need a fresh budget after a stopped turn; leaf agents also
   need read_agent to retrieve long ask_model answers. Add coverage for recovery.
6. Frontend JSX and tests need ordinary formatting; map validation has duplicate
   branches after operation admission. Simplify these without adding dependencies.

The existing module boundaries are suitable: lifecycle in the engine, user
operations in the host, display state in each front end. A larger orchestration
framework would add complexity without improving these contracts.

## Final verification

The dedicated pass is complete. All 1,259 Rust tests passed (3 skipped); workspace
formatting and Clippy passed. TypeScript and all 466 frontend tests passed;
Tauri bridge Clippy passed. The frontend suite retains existing React act warnings
in unrelated components. No live provider calls were needed: model regressions use
scripted local endpoints. Visual artifacts from Chromium are in
`/tmp/oxen-agent-light.png`, `/tmp/oxen-agent-dark.png`, and
`/tmp/oxen-agent-mobile.png`.

Feature commit: `40008e0`. User edits remain outside these commits. Saved Git
baseline refs intentionally remain available for recovery after session deletion;
this is documented in `03-decisions.md`.
