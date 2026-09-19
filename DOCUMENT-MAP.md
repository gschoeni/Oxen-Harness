# Document Map

**Purpose:** Central index of all project files — structure, descriptions, and loading guidance.

---

## Directory Structure

```
oxen-harness/
  00-project-brief.md        — Condensed project context. Load this first. (Tier 1)
  02-status.md               — Phase status, TODOs, what's next. (Tier 2A)
  03-decisions.md            — Working decisions & rationale. (Tier 2B)
  04-backlog.md              — Ideas, links, future exploration. (Tier 2C)
  DOCUMENT-MAP.md            — This file. File index and loading strategy.
  ARCHITECTURE.md            — Crate layering, the lifecycle of a turn, and how to extend.
  PROTOCOL.md                — The wire protocol (SSE events + REST) for building UIs on harness-server.
  AGENTS.md                  — The Ralph Wiggum dev loop + project conventions.
  CONTRIBUTING.md            — Contributor front door: orientation, build/verify, what a good change looks like.
  README.md                  — Public-facing repo README.
  LICENSE                    — Apache-2.0.
  Cargo.toml                 — Cargo workspace manifest.
  rust-toolchain.toml        — Pinned toolchain + components.
  crates/
    harness-core/            — Shared message/role types, defaults, bounded stream text, and copied-around helpers (slug/ellipsize/tail_chars, format_bytes/human_tokens, lenient JSON extraction). Leaf crate.
    harness-config/          — Single source for ~/.oxen-harness paths; atomic + schema-versioned config IO; .env secrets (dotenvy).
    harness-llm/             — Oxen.ai chat client: tool calling + SSE; lightweight auth; attachment store (content-addressed on-disk files) + hydration.
    harness-compress/        — Reversible context compression for tool output: JSON-array crushing, log/line collapsing, CCR store (`<<ccr:hash>>` markers resolved by retrieve_original).
    harness-tools/           — TypedTool trait, bounded process/HTTP capture, fs read/write/edit, glob/search, shell, git, web, questions, canvas, plans, skills, and custom HTTP tools.
                                 fs/ is one module per tool — read (windows + outlines), write, edit (batch hunks, CRLF/BOM), find (glob + grep) —
                                 over shared state.rs (what the model has read, per-path locks, path-scoped conventions), outline.rs (tree-sitter
                                 structural reads), and syntax.rs (did this edit break the file?); edit hunks may be line-addressed (line_start/line_end, insert_after_line).
                                 shell/session.rs carries cwd + env between commands; shell/intercept.rs redirects bare grep/find/cat to the dedicated tools; every command runs
                                 under the non-interactive env (PAGER=cat, CI=true…). tasks.rs: background tasks with settled-task announcements + live output broadcast; steer.rs: the
                                 host's "user spoke mid-turn" signal a foreground wait races against.
    harness-permissions/     — The tool-call gate: tree-sitter shell classification, relaxed/cautious/bypass modes, circuit breakers, trash + snapshots, and the plan-mode latch that holds the tree read-only in every mode.
    harness-store/           — SQLite history (verbatim) + JSONL export; rusqlite_migration schema versioning; rich session metadata; session forks (fork_session/forked_from/user_turns).
    harness-oxen/            — Version config/data + export/share traces via the `oxen` CLI (testable Runner shell-out; no liboxen).
    harness-local/           — Local models: extensible GGUF catalog (Qwen3 + Bonsai), downloads + disk tracking, llama-server launcher.
    harness-theme/           — Configurable themes (palette + voice): built-ins, TOML/JSON load/save with partial overrides, active-theme store.
    harness-agent/           — The agent (Ralph) loop (llm + tools + store); the fleet (run_fleet: N parallel detached subagents, FleetLimits clocks) + the model-facing spawn_agents tool (FleetSpawner/FleetSink);
                                 lane.rs (a lane's typed SubagentResult + the AgentTree of running lanes), lane_tools.rs (send_to_agent / read_agent), tree.rs (the TreeBudget: one billable-token pool per root turn, carved into a wallet per lane),
                                 ask_tool.rs (ask_model: batched tool-less leaf calls), map_tool.rs (map_agents: one lane per item, memoized);
                                 worktree.rs (per-lane git checkouts for editing fleets); config.rs ModelRoles (route work to cheaper models) + RetryPolicy fallback chains;
                                 rules.rs (stream rules: regex corrections that watch the reply and fire only on a match, plus DRAFT_SYSTEM/DraftedRule — model-written rules, self-verified). agent/ splits the loop: turn (the cycle),
                                 call (one model call), tools (gate → waves: shared calls together, exclusive alone; live ToolProgress), repair (heal cut-off JSON, coerce args to the schema), fork (session forks), compaction.
                                 Background tasks that finish are delivered to the model as messages (never polled); every re-call with a corrective emits Nudged; side agents run under a RoundBudget.
    harness-protocol/        — The transport-neutral wire types every UI speaks: the tagged ProtocolEvent enum + command DTOs (serde + JSON Schema); tests/wire.rs is the spec.
    harness-host/            — The transport-agnostic host layer: SessionService (multi-session agent cache, turn driving, question/approval round-trips, model swaps, review/loop runners), generic over an EventSink; the Tauri app and HTTP server are both thin adapters over it.
    harness-server/          — The agent backend as a standalone HTTP server (axum): REST commands + an SSE protocol-event stream with Last-Event-ID replay; bearer-token auth; see PROTOCOL.md.
    harness-runtime/         — Front-end-agnostic services shared by CLI/desktop: connection settings + secrets (.env), cloud-model catalog, tool prefs + custom tools, skill discovery/prefs/authoring, opt-in Oxen versioning of ~/.oxen-harness,
                                 context_files.rs (AGENTS.md/CLAUDE.md/Cursor/Cline discovery → prompt section + path-scoped rules), rules.json discovery (global + per-project stream rules),
                                 commands.rs (custom slash commands from Markdown templates: .oxen-harness/commands + .claude/commands, $ARGUMENTS expansion).
    harness-loop/            — Goal-driven, self-verifying loops (discover→verify→iterate): LoopSpec/Verify, runner, journal, shareable store + built-ins.
    harness-review/          — Configurable code-review pipeline: ordered prompt steps (find→verify→report default), diff targets (uncommitted / vs base branch), isolated side-agent runner (fan-out steps run as a parallel fleet), structured findings.
    harness-cli/             — The `oxen-harness` interactive REPL binary. Slash-command handlers live in commands/ (auth, compression, location, loops, model [+ /model roles], oxen, permissions, plan [/plan read-only mode], preview, queue, resume, rewind [/fork, /rewind], review, rules, theme, trace, ui, usage, print [-p headless]);
                                 custom_commands.rs holds the workspace's Markdown commands; graphics.rs draws inline images (kitty / iTerm2, env-detected) and OSC 8 links; the live sticky-bottom composer in live/ (card.rs: the streaming command-output card + Ctrl+O results;
                                 keys: Esc cancels, Ctrl+Q/Ctrl+Enter queue, Alt+↑ un-queue, Ctrl+O expand); the meters (branch, mode, timer) in turn.rs; the fleet lanes display in fleet_ui.rs/fleet_sink.rs.
                                 Top-level subcommands: theme, loop, trace, oxen.
  app/                       — Tauri v2 desktop app (separate project, excluded
                               from the core workspace). See app/README.md.
    src-tauri/src/           — Rust bridge, a thin adapter over harness-host:
                               lib.rs (module map + run()), state.rs (AppState =
                               SessionService + TauriSink + native-preview hooks),
                               events.rs (the few Tauri-only payloads), commands/
                               (the #[tauri::command] handlers, one module per
                               feature, delegating to the service), cli_open.rs
                               (`oxen-harness ui <dir>` handoff: argv on cold
                               start, single-instance forward when running).
    src/                     — React + TS chat UI (features/, lib/, components/).
                                 features/rules/ — the Rules settings page: RulesPage (data flow), RuleRow (a rule at rest), RuleEditor (+ the live
                                 tester that runs the agent's own regex engine), RuleChat (write a rule by talking to the model), RuleSuggestions, starters.ts.
                                 features/settings/TeachingNav.tsx — the Tools/Skills/Rules trio header shared by all three pages.
  examples/
    web-chat.html            — Dependency-free single-file web client for the HTTP
                               protocol (SSE + REST); the "build your own UI" demo.
  plans/                     — Actionable execution docs. Pull in per-topic.
    archive/                 — Deprecated plans, kept for historical reference.
  RELEASING.md               — Tag/version strategy + the release walkthrough (both paths).
  scripts/                   — Release tooling: version.sh (print), bump-version.sh
                               (rewrite version files + lockfiles), release.sh (tag + push),
                               release-local.sh (build + install CLI/app for this machine, local-* tag),
                               gen-icons.sh (icon set from assets/app-icon.png; inset icns for macOS).
  .github/workflows/         — ci.yml (fmt/clippy/test/doc + frontend + bridge);
                               release-cli.yml (cli-v* tags → 5-platform binaries + release);
                               release-app.yml (app-v* tags → Tauri bundles, draft release);
                               cut-release.yml (one-button bump→commit→tag→build from the Actions UI).
```

## Loading Guide

Context is finite. Load what's relevant, not everything.

### Tier 1 — Always loaded

| Document | Description |
|----------|-------------|
| `00-project-brief.md` | Vision, goals, architecture, current phase. Enough to orient any conversation. |

### Tier 2 — Pull in for working sessions

| Document | When to pull in |
|----------|-----------------|
| `02-status.md` | Any active work — phase status, TODOs, what's next |
| `03-decisions.md` | Implementation work — current decisions with rationale |
| `04-backlog.md` | Planning sessions — ideas, links, future exploration |
| `AGENTS.md` | Any contribution — the dev loop and conventions to follow |

### Plans — Pull in per-topic

| Document | When to pull in |
|----------|-----------------|
| [plans/workbench-views.md](plans/workbench-views.md) | Proposed right-panel extension architecture, unified agent/view tabs, filesystem-backed state, view SDK/packages, migration phases, and acceptance gates |
| [plans/workbench-implementation.md](plans/workbench-implementation.md) | Active implementation checklist, baseline checks, verification and review record for pluggable views |

### Reference — Pull in when you need specifics

| Document | When to pull in |
|----------|-----------------|
| _(none yet)_ | API summaries / research land here |

## Maintenance

When adding a new file to the project, update this document map.

## Subagent hardening (2026-09-13)

- `plans/subagent-hardening.md` — review findings, acceptance checks, and verification record.
- `crates/harness-agent/src/lane_lifecycle.rs` — lane registration cleanup and durable result/workspace finalization.
- `crates/harness-agent/tests/subagent_regressions.rs` — cancellation, construction cleanup, persistence ordering, request admission, and worktree recovery regressions.
- `crates/harness-host/src/agents.rs` — recursive agent history, follow-up, and reviewed patch access/application shared by transports.
- `app/src/features/chat/agents.css` — minimal Agents panel presentation across themes and narrow windows.

## Chat scroll following (2026-09-13)

- `app/src/features/chat/useChatScroll.ts` — scroll intent, layout observation, and return-to-latest behavior; integration tests in `Chat.test.tsx`.
- `plans/chat-scroll.md` — auto-scroll diagnosis, behavior, regression coverage, and review record.

## Error handling audit (2026-09-13)

- `plans/error-handling-audit.md` — propagated-error fixes, remaining lock/FFI findings, regression coverage, and verification record.

## Composer responsiveness (2026-09-18)

- `app/src/features/chat/useComposerSize.ts` — frame-batched textarea sizing for webviews without native content sizing; regression coverage in `Composer.test.tsx`.
- `plans/composer-typing.md` — typing-path diagnosis, browser checks, and review record.
- `app/src/lib/promptInput.ts` — native text-service opt-outs and IME key handling shared by chat and project prompts.

## Media provenance and the gallery's single view (2026-09-18)

- `crates/harness-media/src/library.rs` — `MediaSource` / `SourceOrigin`: each reference traced to the chat attachment, earlier generation, or project file it came from; `agent_prompt` and the hub's `provider` record on every manifest row.
- `app/src/features/media/GalleryPanel.tsx` — the Gallery dock shows the grid or one generation, never both; back button, prev/next stepper, arrow keys.
- `app/src/features/media/GenerationDetail.tsx` — one generation in full: prompt, request, output, the lineage trail (inputs above, outputs below, library items open in place), identity, and the raw manifest row.

## Prairie arcade — 2026-09-18

- `plans/arcade-delight.md` — game-design research, mechanics, browser checks, and review record for all three cabinets.
- `app/src/features/chat/games/arcadeArt.tsx` — deterministic pixel vistas, terrain texture, trees, wagon, and title plaques.
- `app/src/features/chat/games/arcade.css` — cabinet controls outside the playfield, explicit play/pause buttons, and dock sizing.
- `app/src/features/chat/games/{tumbleweed,hunt,oregonTrail}.tsx` — runner Stampede, focused hunting, and the Trail's camp/river decisions; regression tests beside each game.

## Workbench modules and workflows

| File | Purpose |
|------|---------|
| `app/WORKBENCH.md` | User workflow guide, bundled-module SDK, installed package contract. |
| `app/src/workbench-sdk/` | Public view/document API, shared draft store; portable browser SDK lives in `packages/view-sdk/`. |
| `app/src/features/workbench/` | Conversation contexts, registry, host chrome, package manager/native mounting. |
| `app/src/modules/` | Auto-discovered bundled view adapters and Oxen workflow reference. |
| `app/src/modules/oxen-workflow/{NodeCard,NodeInspector}.tsx` | Graph cards, typed handles, model parameters and output previews. |
| `app/src/features/workbench/api.test.tsx` | Late module callbacks retain their originating conversation. |
| `app/scripts/build-workflow-view.mjs` | Builds the graph as an independent installed package. |
| `app/src-tauri/src/view_packages.rs` | Constrained native package surfaces, IPC bridge and asset protocol. |
| `app/src-tauri/src/commands/workbench.rs` | Desktop transport for common workbench actions. |
| `crates/harness-runtime/src/documents.rs` | Revision checks, atomic writes, previous-version recovery, path validation. |
| `crates/harness-runtime/src/workflow.rs` | Versioned graph schema, typed node/edge validation and ordering. |
| `crates/harness-runtime/src/workflow_run.rs` | Explicit execution and durable immutable run snapshots/results. |
| `crates/harness-runtime/src/views.rs` | Host view discovery and file routing. |
| `crates/harness-runtime/src/view_packages.rs` | Reviewed content hashes, local package snapshots and permission manifests. |
| `crates/harness-host/src/workbench.rs` | Session-scoped host tools, Oxen executors, document and workflow transport API. |
| `crates/harness-tools/src/views.rs` | Host-neutral list/open/inspect/run workflow tool contracts. |
| `crates/harness-tools/src/path_lock.rs` | File-write locks shared by independent agent and UI writers. |
| `examples/views/notes/` | Ready-to-install plain-JavaScript document view. |
| `examples/views/workflow/` | The bundled Oxen graph built and installed through the package SDK. |

## Live view authoring

- `packages/view-sdk/` — portable browser contract, injected bridge, draft retention, in-preview tests and SDK tests.
- `crates/harness-runtime/src/view_development.rs` — scaffolding, preview revisions, capability freezes, diagnostic/test reports and promotion.
- `crates/harness-runtime/templates/view/` — no-build starter, theme, tests and agent authoring guide.
- `crates/harness-host/src/view_development.rs` — shared session/agent/transport authoring operations and retained package state.
- `app/src/modules/view-studio/` — create, preview, test, diagnose and install from the current conversation.
- `app/src/features/workbench/PackageSurface.tsx` — native surface lifecycle shared by development previews and installed packages.
- `app/src/features/workbench/PackageSurface.test.tsx` — preview generations remount without rebuilding surfaces during status polling.
- `plans/view-studio.md` — implementation and review record for the live builder loop.

## Shared selectors — 2026-09-19

- `app/src/components/ui/Select.tsx` — default single-choice selector over shared menus; icons, descriptions, keyboard/typeahead and viewport positioning.
- `app/src/components/ui/Select.test.tsx` — selection, accessible descriptions, dismissal, disabled options and keyboard behavior.
- `app/src/components/ui/select.css` — theme-aware selector trigger and menu styling.
- `app/src/features/workbench/Workbench.test.tsx` — module presentation and compatible resource preservation when selecting a view.
- `plans/shared-select.md` — implementation, dedicated review and verification record.
