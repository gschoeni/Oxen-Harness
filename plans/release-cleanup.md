# Release cleanup — 2026-09-30

Goal: ship a release where the core agent works well and nothing half-built is
in the way. Less is more. This plan records what Greg approved from the
launch-readiness audit, in the order it lands. Each numbered step is one or
more small commits; every step ends green (`fmt`, `clippy`, tests; `tsc` +
`vitest` for `app/`).

## Feature flags

`harness_config::features::FeatureFlags` is the one release-switch mechanism.
Every flag is off by default and turned on with an environment variable set to
`1` or `true` before the host starts.

| Flag | Env var | Hides |
|------|---------|-------|
| `workbench_customization` | `OXEN_WORKBENCH_CUSTOMIZATION` | view picker, View Studio, view packages (existing) |
| `workflows` | `OXEN_WORKFLOWS` | the workflow node graph view and the `run_workflow` tool |
| `advanced_settings` | `OXEN_ADVANCED_SETTINGS` | custom HTTP tool editor, code-review step prompts page, composer compression picker |
| `cli_canvas` | `OXEN_CLI_CANVAS` | the `canvas` tool in the terminal |

## Steps

1. **Flags** — add the three new flags above (Rust + `app/src/lib/features.ts`).
2. **Removals (Rust)**
   - `harness-loop` crate, the `loop` subcommand, `/loop` in the CLI and
     desktop, the host/Tauri loop commands.
   - `gh` tool (needs an external CLI; `run_shell` covers it).
   - `git` tool (overlaps `run_shell`; the shell classifier already reviews
     git commands). Own commit so it can be reverted alone.
   - `create_repository` tool and the project-prompt line that advertises it.
     The desktop Repository card and `oxen-harness project` stay.
   - Shell interception: a bare `grep`/`cat`/`find` now just runs.
   - Model-drafted stream rules (`/rules draft`, the drafting backend, the
     Tauri draft commands). Manual rules and the suggestion gallery stay.
   - Local models "Oxen" tab stub (`oxen_featured`).
   - Orphaned Tauri commands (`fs_write_file`, `install_llama`).
3. **Removals and gating (frontend)**
   - Remove `/loop`, `RuleChat`, the "Oxen" tab, dead `ipc.ts` exports.
   - Workflow view behind `workflows`; C10 controls behind `advanced_settings`.
   - The study cabinet is the default hero game.
   - Ask for an API key up front on first run instead of after a 401.
4. **Flag wiring (Rust)** — `run_workflow` behind `workflows`; `canvas` in
   the CLI behind `cli_canvas`.
5. **Bug fixes**
   - A1: replies capped at 4,096 output tokens → use the model's real limit.
   - A2: `start_dev_server` commands go through the shell permission review.
   - A3: the harness's own API keys are stripped from child-process envs.
   - A4: stale "trail" text in the `gh` description (moot: tool removed).
   - A5: desktop CSP and asset-protocol scope tightened.
   - A6: plain missing-key error in the CLI; up-front key prompt on desktop.
   - A7: refresh the fallback context-window table.
   - A8: usage-ledger and context-snapshot write failures are reported.
   - E6: `web_fetch` refuses link-local addresses (cloud metadata).
6. **Tool descriptions and prompt** — rewrite every remaining tool
   description and schema doc comment to be short and effective; remove
   prompt text that repeats a tool description; lower the schema budget test
   to the new size.
7. **Panic safety (D7)** — replace lock-poison `expect`/`unwrap` in
   production code with propagated errors; deny `clippy::unwrap_used` and
   `clippy::expect_used` workspace-wide (tests exempt); state the rule in
   `AGENTS.md`.
8. **Docs (E)** — README, status, backlog, brief, document map, architecture:
   delete stale or wrong text, describe what actually ships, add an install
   path, state the default permission mode and that the shell is
   workspace-scoped rather than sandboxed. Delete `erl_crash.dump`, ignore the
   pattern.
9. **Polish pass** — review the whole diff, apply what is worth it, re-verify,
   commit separately.

## Not in this pass (audited, not approved)

Flagging off media, fleet, preview or view tools (B1–B5); registering
`web_search` only with a key (B7); the intent nudge (B12); the conventions cap
(B13); the arcade itself (C1 — replaced by making study the default);
vibe-coded themes (C5); training-data import (C6); the `trace`/`oxen`/`project`
subcommands (D1); CI on macOS/Windows (E4).

## Progress log

- 2026-09-30: plan written; flags added.
