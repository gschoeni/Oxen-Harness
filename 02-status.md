# Project Status & Roadmap

**Purpose:** Where we are, what's next, what's done. Pull this in for any working session.
**Updated:** 2026-07-24

---

## Phase Overview

| Phase | Goal | Status |
|-------|------|--------|
| **0** | Scaffold: workspace, crates, first green tests, KB, license | ✅ Complete |
| **2** | `harness-tools`: fs read/write/edit/search, sandboxed shell, git, web search, ask_user_question | ✅ Complete |
| **3** | `harness-store`: SQLite history (verbatim) + JSONL export | ✅ Complete |
| **1** | `harness-llm`: Oxen client — tool-calling types, auth, SSE streaming | ✅ Complete |
| **4** | `harness-agent`: the agent (Ralph) loop | ✅ Complete |
| **5** | `harness-cli`: interactive streaming REPL | ✅ Complete |
| **6** | `app/`: Tauri v2 cross-platform desktop app | ✅ Scaffolded (compiles) |
| **7** | `harness-local`: extensible local GGUF models via llama.cpp | ✅ Complete |
| **8** | `harness-theme`: configurable + shareable themes (palette + voice) | ✅ Complete |
| **9** | `harness-loop`: goal-driven, self-verifying loops (discover→verify→iterate) | ✅ Complete |
| **10** | `harness-compress`: reversible context compression (off/audit/on) | ✅ Complete |
| **11** | `harness-review`: configurable code review — `/code-review` (find→verify→report, editable step prompts), desktop Settings page + Review button | ✅ Complete |
| **12** | Fleet: parallel subagents — `fleet::run_fleet` + `spawn_agents` tool (all modes), review find fan-out (3 lenses), live lanes w/ watch-a-lane in TUI (1-9/alt+1-9) and desktop panel | ✅ Complete |
| **13** | Cleanup pass: shared helpers → `harness-core` (text/fmt/json), CLI handlers → `commands/`, desktop bridge split (state/bridges/events/commands), a `/code-review` self-review with 17 fixes | ✅ Complete |
| **14** | Usage accounting: provider/fallback tokens per model call, daily activity ledger, estimated Oxen spend, desktop activity grid + CLI `/usage` | ✅ Complete |
| **15** | Long-running memory hardening: bounded streaming I/O, durable context checkpoints/CCR payloads, attachment budgets, bounded fleet/UI caches | ✅ Complete |
| **16** | Durable projects: guided creation, repo-local goals/instructions/context, project getting-started/settings page | ✅ Complete |

> Build order note: independent crates (tools, store) were built before the LLM
> client to keep each phase fast to verify. The agent loop lives in its own
> `harness-agent` crate (not `harness-core`) to avoid a dependency cycle.
> **652 Rust tests + 230 frontend tests passing**; CI runs fmt + clippy + tests + docs on the workspace, and tsc + vitest + bridge-clippy on the desktop app, on every push.

## Work panel release scope — 2026-09-28

Work-panel customization now defaults off. The view picker, welcome shortcuts,
View Studio, installed packages and authoring operations share the host's
`OXEN_WORKBENCH_CUSTOMIZATION` startup flag. Built-in file viewers and saved
workflow graphs remain available; old customization contexts fall back without
deleting users' files or packages. The flag can be enabled for hardening work
without rebuilding. Review also hardened fallback file navigation and preserved
forward history on startup. Verified with 1,399 Rust tests, 678 frontend tests,
formatting, both Clippy checks, TypeScript and the production build. See
`app/README.md` and `plans/workbench-release-flag.md`.

## Shared view selector — 2026-09-19

The work-view picker now uses a shared `Select` over the existing menu primitives:
module icons, descriptions, selected-state checks, keyboard/typeahead, and a
viewport-aware portal. It preserves compatible open files and uses the same
native-overlay behavior as the other app menus. `app/README.md` documents it as
the default for new single-choice selectors. Dedicated review fixed dynamic
option removal and upward menu positioning. Verification: 633 frontend tests,
TypeScript, production build, browser checks, formatting and native Clippy passed.
Nextest passed 1,368 tests (5 skipped, one existing test reported leaky). Workspace
Clippy remains blocked by an unrelated `too_many_arguments` warning in
`harness-agent/src/agent/tools.rs::run_repaired` during the `CallContext` refactor.
See `plans/shared-select.md` for the review and verification record.

## Live View Studio — 2026-09-18

Built a no-recompile package authoring loop: project scaffolds, live immutable
previews, agent tool, bounded runtime diagnostics, registered browser tests,
retained drafts, exact-revision installation, and a portable SDK. The dedicated
review/refactor is complete: same-code restarts remount correctly, source-specific
reports survive restarts, previous-preview rollback leaves source untouched,
native assets remain immutable in memory, and document switching protects drafts.

Final verification: fmt and workspace/native Clippy passed; 1,357 Rust tests
passed under nextest (5 skipped), 626 frontend tests passed, TypeScript, four SDK
tests, native command isolation, desktop assets and standalone workflow build
passed. Chrome smoke covered draft retention, concurrent-agent save conflicts,
registered tests/failure reports, light/dark themes and a 340-pixel panel without
horizontal overflow. Native interactive platform coverage and paid generation
remain untested. See `plans/view-studio.md` and [the builder guide](app/WORKBENCH.md).

## Workbench views and Oxen workflows — 2026-09-18

Implemented bundled module discovery, a public view SDK, conversation-owned
view navigation, revision-aware document drafts, and reviewed installable local
view packages. The first custom module is a drag/drop Oxen node graph covering
prompt rewriting, images, video, image/video upscaling, and collected outputs.
Agent tools can discover, open, inspect and explicitly run saved graphs. Run
snapshots/results are durable; edits never execute automatically. Canvas
artifacts now persist as project files. See [the guide](app/WORKBENCH.md).

Dedicated review/polish is complete. Final verification: 1,350 Rust tests passed
under nextest (5 skipped), 622 frontend tests passed, TypeScript, workspace/native
Clippy and formatting passed. The native command-isolation regression passed.
Browser verification exercised palette drag/drop, node movement, settings and
saving with no page errors and no independent right-panel tabs. Desktop assets
and the independent workflow package both build. Live paid generation and native
installed-view rendering on Windows/Linux have not been exercised.

## Phase 16 — Durable projects

**Status:** ✅ Complete

- [x] Replace the folder-only action with a guided **Start a project** flow for
      creating a new directory or adopting an existing one.
- [x] Keep creation focused on name, goal, and folder; persist an optional
      default parent directory for future new projects.
- [x] Persist project name, goal, instructions, and a context manifest in the
      repository at `.oxen-harness/project.json`; folder-only projects migrate
      implicitly with their directory basename and empty metadata.
- [x] Copy text/PDF/image references content-addressed into
      `.oxen-harness/context/`, with add/remove/deduplication behavior.
- [x] Add a model-selectable project getting-started/settings page with editable
      inline name/goal plus Instructions and Context cards. New projects land
      there; established projects resume their newest chat and expose the page
      from a files button in the chat titlebar.
- [x] Feed goals/instructions/context manifests into both desktop and CLI agent
      prompts; attach durable PDF/image context to the first prompt of new chats.

## Phase 15 — Long-running memory hardening

**Status:** ✅ Complete

- [x] Drain shell, git, verification, HTTP, and file streams incrementally with explicit bounds.
- [x] Checkpoint compacted active context in SQLite so cold resume avoids rebuilding the full transcript.
- [x] Bound resident context, attachment hydration, compression caches, fleet channels, and side-agent persistence.
- [x] Keep CCR originals on disk for workspace-backed agents instead of in heap.
- [x] Bound desktop session/tool caches, batch token events, and skip offscreen thread rendering.
- [x] Preserve full history on disk while truncating only transient display projections.

---

## Phase 0 — Scaffold

**Status:** ✅ Complete

- [x] Workspace `Cargo.toml` with shared workspace deps
- [x] Five crate skeletons (`core`, `llm`, `tools`, `store`, `cli`)
- [x] First green tests in each crate (role wire format, URL builder, tool trait, JSONL export)
- [x] Apache-2.0 `LICENSE`, `.gitignore`, `rust-toolchain.toml`
- [x] `README.md` + `AGENTS.md` (Ralph loop as the dev process)
- [x] Knowledge base filled in (`00`/`02`/`03`/`04`/`DOCUMENT-MAP`)
- [x] Verification loop green (`fmt`, `clippy`, tests)
- [x] `git init` + initial commit

---

## Phase 2 — harness-tools

**Status:** ✅ Complete (35 tests passing)

- [x] `Workspace` sandbox: path resolution rejecting escapes outside the root
- [x] `Tool` trait, `ToolRegistry` (dispatch by name), OpenAI tool definitions
- [x] `TypedTool` (2026-07-01): args are a typed struct, the schema derives from
      it via `schemars` (doc comments = model-facing descriptions) — replaced
      the hand-written schemas + `args` extraction helpers across all tools
- [x] fs tools: `read_file`, `write_file`, `edit_file` (unique-match), `search_files`
- [x] `run_shell`: command execution pinned to workspace root
- [x] `git`: status / diff / log / commit
- [x] `web_search`: Brave Search API (registered only when `BRAVE_API_KEY` is set)
- [x] `ask_user_question`: interview the user with 1–4 multiple-choice questions
      (Claude Code `AskUserQuestion` shape: `header`/`question`/`options`/`multiSelect`).
      Rendering is host-specific via a `QuestionAsker` trait — the CLI draws an
      interactive `crossterm` picker; the desktop app shows a question card.

**Tooling parity pass** (2026-06-21) — researched Claude Code's essential tool set
and closed the obvious gaps (no MCP, no orchestration/network tools):

- [x] `read_file` now returns `cat -n` line numbers + `offset`/`limit` + truncation caps
- [x] `find_files` (Glob): find files by glob pattern, gitignore-aware, newest-first
- [x] `search_files` (Grep): regex search with `content`/`files_with_matches`/`count`
      output modes, `glob`/`path` filters, and `case_insensitive`
- [x] `run_shell`: `timeout_ms` (default 120s) + 30k-char output cap to prevent hangs/blowups
- [x] System prompt updated to steer toward dedicated tools + read-before-edit

---

## Phase 3 — harness-store

**Status:** ✅ Complete (7 tests passing)

- [x] SQLite schema: `sessions` + `messages` (verbatim `raw_json`, per-session `seq`)
- [x] `create_session` / `append_message` (any serializable message) / `messages`
- [x] Tool-call messages stored and read back verbatim
- [x] `export_jsonl` (one verbatim message per line) for fine-tuning
- [x] Persists across reopen

---

## Phase 1 — harness-llm

**Status:** ✅ Complete (14 tests)

- [x] OpenAI-compatible request/response types (incl. `tools`, `tool_calls`, `tool_choice`)
- [x] Auth resolution: `OXEN_API_KEY` → parse `auth_config.toml` by host (no `liboxen`)
- [x] Non-streaming chat completion call (mocked with `mockito`)
- [x] SSE streaming of assistant tokens (`SseDecoder` + `StreamAssembler`)
- [x] Tool-call parsing + streamed tool-call fragment merging

---

## Phase 4 — harness-agent

**Status:** ✅ Complete (1 integration test exercising the full loop)

- [x] `Agent` wires `OxenClient` + `ToolRegistry` + `HistoryStore`
- [x] Ralph loop: stream model → run tool calls → append `tool` messages → repeat → stop
- [x] `AgentEvent` surfaces tokens + tool start/end for live UIs
- [x] Every message persisted verbatim as produced
- [x] Scripted-mock integration test: tool call then final answer

---

## Phase 5 — harness-cli

**Status:** ✅ Complete (39 tests; binary verified)

- [x] `oxen-harness` binary with clap args (`--model`, `--workspace`, `--base-url`,
      `--host`, `--resume`, `--local`) + `models` subcommand group
- [x] Interactive REPL (rustyline) with live token streaming to stdout
- [x] **Oregon-Trail themed UI** (`theme.rs`): 24-bit color, "OXEN TRAIL" ASCII
      wordmark + covered-wagon banner, "size up the situation" trail journal
- [x] In-place animated spinner with rotating trail verbs ("Fording the river…",
      "Yoking the oxen…") + elapsed time, Claude-Code style (no flicker)
- [x] **Streaming Markdown renderer** (`markdown.rs`): headings, bold/italic, inline
      `code`, lists, blockquotes, rules, links, and fenced code blocks rendered live
      (line granularity); GFM tables buffered + drawn as aligned box-drawn grids
      (with `:--`/`--:`/`:-:` alignment); tombstone "you have died of…" screen on quit
- [x] Themed tool lines (`◆ verb  name(args)` / `└─ result`) and death-message errors
- [x] Color auto-disabled for non-TTY / `NO_COLOR` / `TERM=dumb` (piped output stays clean)
- [x] Slash commands themed as the game menu: `/help`, `/model [name]`, `/export [path]`, `/exit`
- [x] Sessions persisted to `~/.oxen-harness/history.sqlite`
- [x] **Resume by id** (`--resume <SESSION_ID>`): the death screen engraves the
      session id + resume command; resuming restores the saved transcript,
      workspace, and model (overridable with `--workspace` / `--model`)
- [x] **Local models**: `models list/pull/remove/path` subcommands (themed table +
      Oregon-Trail download progress bar) and `--local <id>` to run a downloaded
      model through `llama-server` for the session
- [x] **Interactive clarifying questions**: a Claude-Code-style picker, now in a
      reusable `picker.rs` module (single/multi-select, number jumps, "type my own
      answer" row, `esc`/`Ctrl-C` cancel; raw mode via RAII guard; `spawn_blocking`;
      non-TTY fallback). `ask.rs` delegates to it; `/theme` selection reuses it.
- [x] **Themes** (`theme.rs` reads `harness_theme::Theme`; `commands/theme.rs`): `/theme`
      opens the picker, `/theme use|import|export`, and `/theme new [vibe]` runs a
      short interview + model generation to vibe-code a theme. Top-level
      `oxen-harness theme list|use|export|import|path|remove` for sharing/scripting.
      Theme switches hot-swap the live UI.
- [x] Graceful, helpful exit when no API key is configured
- [x] **Live sticky-bottom composer** (`live.rs`): on an interactive TTY, a
      composer pinned to the bottom row lets the user type while a turn streams.
      Turn output scrolls inside a DECSTBM region above it; submitted lines stack
      onto the `MessageQueue` (prompt shows `[n queued]`) and auto-drain in order
      when the turn ends. Raw-mode Ctrl-C interrupts, Ctrl-D exits. Gated behind
      `is_terminal() && animates()`; pipes/`NO_COLOR`/`TERM=dumb` keep the classic
      blocking prompt. Composer line-editing is a pure, unit-tested `Composer`.

---

## Phase 6 — Tauri v2 desktop app (`app/`)

**Status:** ✅ Scaffolded; Rust bridge compiles + clippy-clean

- [x] Separate Cargo project (excluded from the core workspace) so core stays fast
- [x] `src-tauri` bridge: `run_turn` + `session_info` commands over `harness-agent`
- [x] Live streaming to the UI via `agent://token` / `agent://tool` events
- [x] Dependency-free chat frontend (Cursor-agents-style) using `withGlobalTauri`
- [x] **Local models in the UI**: `list_models` / `pull_model` / `remove_model` /
      `use_local_model` commands; a "🐂 Local models" modal lists the catalog with
      disk usage, downloads with a live progress bar (`models://progress`), and
      switches the session to a local model
- [x] **Clarifying questions in the UI**: `ask_user_question` emits an
      `agent://question` event; the frontend shows a question card (radio /
      checkbox options + a free-text row) and `answer_question` unblocks the
      agent via a per-question channel
- [x] **Themes in the UI**: `list_themes` / `active_theme` / `use_theme` /
      `import_theme` / `export_theme` / `remove_theme` / `new_theme` commands; a
      "🎨 Theme" panel selects themes (applying the palette to CSS variables and
      using the voice phrases), vibe-codes a new theme via the model, and
      imports/exports shareable theme files
- [x] **Desktop slash commands**: typing `/` opens a keyboard-navigable command
      list; the CLI command families dispatch to native desktop actions without
      reaching the model (desktop intentionally omits `/exit`). `/loop` has full
      list/show/new/run/goal/import/export/remove/path parity and runs through
      the shared cancellable `harness-loop` runner.
- [x] Tauri v2 capability granting `core:default`; valid app icon
- [ ] Run-time GUI verification (needs a desktop session + API key; `cargo tauri dev`)
- [ ] App icons for bundling + enable `bundle.active` for installers

---

## Phase 7 — harness-local (local models via llama.cpp)

**Status:** ✅ Complete (53 tests passing)

- [x] Config-driven `catalog`: curated Qwen3 GGUFs (`Q4_K_M`) from 0.6B →
      32B + 30B-A3B MoE, plus Bonsai 27B binary (`Q1_0`) and mainline-compatible
      ternary (`Q2_g64`) builds, with exact HF repo/file/size/context metadata
- [x] New model families default to one exact artifact; the standard Q8→Q3
      filename ladder is opt-in (`derive_quants`) so the catalog never invents
      unsupported downloads. User additions/overrides live in
      `~/.oxen-harness/local-models.json`.
- [x] `ModelStore`: `~/.oxen-harness/models/` dir — installed status, per-model +
      total disk usage, streaming download (atomic `.part` → rename) with progress,
      and remove
- [x] `LocalServer`: locate `llama-server` (`LLAMA_SERVER` override, managed
      runtime, or `PATH`), pick a free port, spawn against a GGUF (`--jinja` for
      tool calling), poll `/health` until loaded, and kill on drop (no leaked
      background server). The managed Apple Silicon runtime is pinned to
      llama.cpp `b10353` for Muse Glimmer support (Bonsai Q1/Q2 already worked
      on `b10002`).
- [x] Talks to the agent as just another OpenAI-compatible endpoint
      (`http://127.0.0.1:<port>/v1`, throwaway key) — no client changes needed
- [x] Downloads managed in-process (not delegated to `llama-server --hf`) so both
      the CLI and UI can show real progress + disk usage

---

## Phase 8 — harness-theme (configurable + shareable themes)

**Status:** ✅ Complete (15 tests passing)

- [x] `Theme` model: `Meta` + `Palette` (7 terminal colors + app bg/surface/border,
      each a `#rrggbb` `Color`) + `Voice` (prompt, spinner glyphs, thinking phrases,
      per-tool verbs, deaths, banner art/wordmark/labels, help items, exit art)
- [x] TOML (and JSON) load/save with **partial overrides** via deep-merge over the
      default, so a theme file (hand-written or model-generated) can set just a few
      fields; `to_toml` for export; `from_model_output` tolerates fences/prose
- [x] Built-ins: **Oregon Trail** (default), **Midnight**, **Synthwave**,
      **New York Times**, **Cupertino** — each with its own `[style]` (fonts,
      framing, hero layout) so they look genuinely different, not just recolored
- [x] `Store` under `~/.oxen-harness/`: `config.toml` active slug + `themes/<slug>.toml`;
      list (built-ins + installed, installed shadows built-in), resolve, set_active,
      save, import, export, remove; filesystem-safe slugs
- [x] Consumed by the CLI (`theme.rs` renders from the active `Theme`; `Ui` carries
      `Arc<Theme>`) and the desktop app (palette → CSS variables, voice phrases)
- [x] Vibe-coding: a short interview feeds the model `Theme::generation_system_prompt()`
      (schema + default as reference); output parsed, saved, and activated

---

## Phase 9 — harness-loop (goal-driven, self-verifying loops)

**Status:** ✅ Complete (16 tests passing; CLI wired)

- [x] `LoopSpec` + `Verify` (TOML): a **command** gate (shell exit 0 = pass) or a
      strict **rubric** gate (separate-checker scores 1–10 vs. criteria, threshold);
      `success_criteria`, `max_iterations` (default 8), optional `token_budget`
- [x] **Conditional gates** (2026-07-06): verify is a list of *named* gates, each
      with `run_when` (`always`, or `on_change` + glob patterns); a git content
      snapshot skips e.g. the test gate when no matching code changed, while
      failed/blocked gates always re-run
- [x] `LoopRunner`: drives DISCOVER→QUESTION→PLAN→EXECUTE→VERIFY→ITERATE — each
      pass composes goal + criteria + a journal digest of prior attempts, runs one
      agent turn (tools + `ask_user_question`), then runs the gate; stops on success,
      iteration cap, token budget, or agent error. The gate (not the model) decides.
- [x] `LoopJournal`: per-iteration record (summary + verify outcome), persisted to
      `~/.oxen-harness/loops/runs/<slug>.json` after each pass for resumability
- [x] `LoopStore`: shareable loops as `~/.oxen-harness/loops/<slug>.toml` (installed
      shadows built-in); built-ins `default` (fmt+clippy+test), `green-tests`, `clean-clippy`
- [x] CLI: `oxen-harness loop run|list|new|show|import|export|remove|path` and in-REPL
      `/loop run|goal|list|new|show|…`, reusing the shared `render::TurnRenderer`
      (extracted from `main.rs`) with Ctrl-C interrupt support
- [ ] Loop support in the Tauri desktop app (follow-up pass)

---

## Recent — extensibility push (2026-07-01)

Tools, skills, and the surfaces to manage them, in one sweep:

- **TypedTool refactor** (`harness-tools`): schemas derive from typed args
  structs — advertised interface and parsed arguments can't drift; registry
  completeness + schema-budget tests; "Adding a built-in tool" recipe in AGENTS.md.
- **Custom HTTP tools**: name + description + JSON-schema params + endpoint;
  arguments POST as JSON, response body = tool result. Settings → Tools editor
  with a simple parameter builder (JSON mode for complex schemas).
- **Skills** (Claude Code shape): `SKILL.md` dirs, global
  (`~/.oxen-harness/skills/`) + per-project (`.oxen-harness/skills/`, committed
  — see the repo's own `add-a-tool` skill). Progressive disclosure via a single
  `skill` tool; Settings → Skills page to create/edit/toggle; README
  "Extending the agent".
- **Host parity**: the CLI now applies tool prefs + skills like the desktop;
  both hosts gate the system prompt on the tools that actually survived
  preferences (canvas was hardcoded before).
- **Desktop navigation**: projects became a full-window picker page; the
  sidebar scopes to one project's chats.

---

## Recent — resilience push (2026-07-05 → 07)

Context growth, flaky endpoints, and recovering dead turns:

- **Context compression** (`harness-compress`, 2026-07-05): reversible
  compression of stale tool output before it goes on the wire — off / audit
  (measure, change nothing) / on. Compressed content leaves a `<<ccr:hash>>`
  marker the model can resolve with the `retrieve_original` tool; the
  transcript and store always keep the originals. Savings surface in the CLI
  meter, the desktop TokenMeter, and Settings.
- **Context compaction** (2026-07-05): instead of hard-stopping on
  `ContextWindowExceeded`, the agent prunes stale tool output, then summarizes
  the oldest turns (on user-turn boundaries) — the session continues with a
  `Compacted` event; the store keeps the full record.
- **Model-call retry** (2026-07-06): transient provider/network failures
  (5xx, rate limits, dead streams) retry with exponential backoff
  (`RetryPolicy`, default 4 attempts from 1s), emitting `Retrying` events so
  the UI shows a hiccup, not a hang; exhausted retries report attempts +
  model + endpoint (`RetriesExhausted`). Non-transient errors fail fast.
- **Recovery UX** (2026-07-06): `/retry` re-drives a transcript that stopped
  mid-turn without duplicating the user message (`Agent::continue_turn`);
  `--continue` reopens the newest session; both terminals print the same
  failure report with the way out.
- **Loop conditional gates** (2026-07-06): see Phase 9.
- **`/model` picker** (2026-07-07): `/model` with no argument opens the
  interactive picker (cloud catalog + installed local models, current marked);
  the live composer completes `/model <partial>` against ids *and* display
  names, and Enter accepts the highlighted completion. Unknown ids are saved
  as custom catalog entries.
- **Module shape pass** (2026-07-07): the three files that had grown past
  ~1200 lines were split by concern with no API changes — `harness-agent`
  (error/event/config + `agent/{turn,compression}`), the CLI's `live/`
  (turn/events/paint/completion), and `main.rs` (endpoint/turn/repl_loop +
  `model_cmd`/`compression_cmd`).

## Recent — model usage reporting (2026-07-11)

- Every completed model call records timestamped prompt/completion tokens under
  its model and endpoint source. Provider counts win; unsupported endpoints use
  the existing calibrated estimate. Review agents, fleet lanes, one-shot model
  helpers, tool-loop calls, and partial replies all share the same ledger.
- Settings → Usage has a theme-aware yearly activity grid with daily hover
  totals, year navigation, and click-to-filter stats/model bars for one day.
- The desktop hero and CLI banner show all-time tokens and estimated spend;
  `/usage` prints the CLI's per-model input/output/cost table.
- Dollar figures use rates advertised by the configured Oxen-compatible
  endpoint's model catalog and are labeled estimates. Models without published
  rates remain explicitly unpriced rather than `$0.00`.

## Recent — data grid in the Editor pane (2026-07-16; removed 2026-07-26 — cut as bloat along with the Polars dependency; data files open in the plain code editor)

- CSV/TSV/JSONL/Parquet files open as an Airtable-style virtualized grid in the
  desktop Editor dock (`app/src/features/files/DataView.tsx`), with server-side
  paging/sorting/searching in a Polars-backed Tauri command module
  (`app/src-tauri/src/commands/dataset.rs`) — the webview only ever holds the
  visible ~200-row pages, so million-row files on disk scroll interactively.
- Cells edit in place (dtype-validated) and write back surgically: CSV/JSONL
  rewrite only the touched record byte-for-byte; Parquet rewrites the file
  under a 256 MB cap and is read-only beyond it. CSV/TSV/JSONL keep a Raw
  toggle into the ordinary code editor.

## Recent — parity pass against oh-my-pi and pi (2026-07-24)

Branch `harness-parity-improvements`; the study and full roadmap are in
`plans/harness-parity-improvements.md`, whose Progress log tracks what shipped.

- **Project conventions in the prompt**: `AGENTS.md`/`CLAUDE.md` (plus Cursor,
  Cline, and Copilot rule files) are discovered and folded into the system
  prompt inside the cached prefix; path-scoped rules ride along on the first
  touch of a file they govern. `/context` shows what was loaded.
- **File-editing safety**: edits are gated on a prior read, refused when the
  file moved on disk since that read, and serialized per path — the last of
  which is what makes parallel fleet lanes survivable at all.
- **`edit_file` takes a batch** of replacements (matched against the original,
  all-or-nothing), and preserves CRLF/BOM.
- **Truncated `run_shell` output is retrievable** rather than destroyed, via
  the existing CCR store; `retrieve_original` is now always registered.
- **Structural reads**: a whole-file read of a big source file returns an
  outline with function bodies elided, and editing into an elided range is
  refused with the range to re-read.
- **Model roles + fallback chains**: `smol`/`summary` route work off the
  session model; a model that keeps failing transiently hands the call to the
  next configured one instead of ending the turn.
- **`run_shell` remembers `cd` and `export`** between calls.
- **Post-edit syntax check**: an edit that leaves the file unparseable says so
  on the tool result, comparing before/after so an already-broken file isn't
  blamed on the edit that touched it.
- **Editing fleets isolate** in per-lane git worktrees and return patches.
- **Stream rules**: user/project regex corrections that watch the reply as it
  streams and interrupt it on a match, with a Settings → Rules page whose
  tester runs the agent's own regex engine (never the browser's) so what you
  see is what will fire. The terminal gets the same capabilities via `/rules`
  (list, guided add, on/off, rm, test), applied to the live session. Written up
  in the README under "Extending the agent". A shared suggestion library
  (`harness_runtime::rules::suggestions`) backs both the desktop gallery and
  `/rules suggest`, so a new user starts from explained examples rather than a
  blank page and a regex field. Rules can also be *drafted* by the model from a
  plain-language description (Settings → Rules, or `/rules draft`), with the
  draft verified against the model's own example and counter-example before it
  is offered. Drafting is a conversation: tokens stream to the editor
  (`rules://draft`), each proposal shows the check that ran on it, and
  follow-ups revise the rule on the table rather than starting over.

## Recent — smoothness pass from the oh-my-pi study (2026-09-01)

A deep read of oh-my-pi (report: the "omp Field Notes" artifact) turned into a
port of the patterns that make an agent feel smooth, all green under
fmt/clippy/nextest (1107 tests) and verified at the pty surface:

- **Loop** — tool calls in one reply run in waves (shared together, exclusive
  alone; `Tool::concurrency`), results folded in call order, `call_id` on
  ToolStart/ToolEnd/protocol. Background tasks auto-deliver their output as
  messages (no polling); a mid-turn user message backgrounds a running shell
  command early (steer channel). Argument repair (heal cut-off JSON, coerce
  to the schema), empty results spelled out, loop guard canonicalizes args,
  stage-1 compaction prunes only what the overflow needs, side agents run
  under a RoundBudget (40/60), every corrective re-call emits `Nudged`.
- **Tools** — non-interactive shell env, grep/find/cat interception, 60 s
  patience, `task_output wait_ms`, live output broadcast, line-addressed
  edit hunks (hashline-inspired), `update_plan`/`ask` hygiene lines.
- **CLI** — streaming command-output card + Ctrl+O, richer sealed summaries,
  Esc cancels, Ctrl+Q/Ctrl+Enter queue + Alt+↑ un-queue, multi-line pastes +
  paste chips, meters with branch/mode/timer/colour-graded fill, terminal
  title + bell, instant startup (pricing off the critical path), recent trails
  in the banner, `/resume` picker + bare `--resume`, `-p` print mode, custom
  Markdown slash commands, `/fork` + `/rewind` on session forks, prompt
  history keeps newlines.
- **Store/app** — session forks (migration 11); the desktop app pairs tool
  chips by `call_id`, appends live output, and shows `agent.notice` lines.

Second round (same day): `@path` completion, double-Esc rewind, `/help` key
table, `spawn_agents wait: false` with aside delivery (registry `Asides`),
loop guard catches interleaved repeats, compaction summaries carry a
`<files>` block, plan mode (gate latch + `/plan on|off|approve|show`),
`/model roles` + a first-run model pick, quiet `-p` when local models
narrate, and a real bug: the kitty-keyboard probe cost 2 s on every prompt
on terminals that never answer (Apple Terminal, ptys) — now once per
process. pty-verified: plan mode refuses a write with a clear message; a
background fleet's report is delivered on the next round.

Images (2026-09-07, from omp's composer/transcript image handling):
downscale to 1568 px / upscale under 200 px before send with dimensions on
the attachment, Finder file-list before bitmap on paste, atomic chip
deletion, `@image.png` mentions attach, `-p` attaches, inline thumbnails on
attach and for tool-produced images (`graphics.rs`: kitty + iTerm2 by env,
`OXEN_HARNESS_IMAGES=off`), `read_file` on an image attaches it for the
model (marker + size line), OSC 8 links, Ctrl+G external editor.

Deliberately not copied: sixty providers, yolo-by-default approvals, magic
keywords, vibe/goal modes, IRC between subagents, in-process TS extensions.

## Recent — fleet hardening, phase 0 of the recursive fleet plan (2026-09-13)

The first phase of the Recursive Fleet Plan (the September study of Claude
Code, Codex, oh-my-pi, Prime's RLM harness and the RLM paper against our
`spawn_agents`): plug the holes before adding depth. Nothing new for the
model to call yet.

- **One subagent config.** `AgentConfig::for_subagent` decides everything a
  lane inherits differently (smol role, non-interactive gate, round budget,
  no re-attached project binaries, trail-free prompt); `side_agent` and the
  fleet spawner both use it. A `spawn_agents` lane previously had no round
  budget at all and re-uploaded every project PDF.
- **Clocks.** `FleetLimits`: a 10-minute lane time limit and a 30-minute
  fleet deadline beside the concurrency cap. Stopped lanes keep what they
  streamed, marked with a `LaneStop`; a lane that ignores its stop for 20 s
  is abandoned. The fleet stops on its own child token, never the caller's.
- **Bounded results.** `combine_outcomes_capped` gives each lane 12k chars
  (or its share of 48k) and parks the whole reply in the registry's overflow
  store behind a `<<ccr:HASH>>` marker for `retrieve_original`. The review
  pipeline keeps whole replies (they feed a fresh side agent).
- **Named, stoppable, bounded fleets.** Every `fleet.*` event and `FleetSink`
  call names its fleet; the host batches lane tokens per (fleet, lane), the
  CLI hub keeps every fleet in flight and paints the oldest, the desktop
  shows one panel per fleet with a Stop button. `cancel_fleet` /
  `POST /v1/sessions/{id}/fleets/{fleet}/cancel` stops one fleet without
  ending the turn. At most three fleets per session in flight.
- **Classification.** `spawn_agents` is `Exclusive` (mutating to the gate,
  alone in its wave); plan mode admits it by name.
- **Attribution.** Lane and review spend is recorded against the spawning
  session (`HistoryStore::usage_for_session`), the seam a tree-wide budget
  hangs off next. Lane shells share the overflow store.

Next phases, in order: a subagent as a persisted, resumable object with a
typed result; depth 2 with a shared tree budget; context-as-variable
(handles for big tool outputs, batched tool-less leaf calls); a declarative
`map_agents`; fork mode; an agent hub.

## Recent — recursive fleets, phases 1–6 of the plan (2026-09-13)

The rest of the Recursive Fleet Plan, same day. A subagent is now an object
with a name, a typed result, a transcript, and a budget, and the tree can
go one level deeper without losing its wallet.

- **A lane is a persisted, resumable object.** A lane is a session under
  `sessions.parent_session` (M13): transcript kept, hidden from every chat
  list, deleted with its parent. Its id is what everything addresses it by:
  `send_to_agent` resumes it with its context for a follow-up, `read_agent`
  reads its full reply (lines / grep), `GET …/agents` lists a session's
  lanes, `…/agents/{agent}/cancel|interject` reach one running lane (the
  live registry, `AgentTree`). `SubagentResult` replaces the prose document:
  done / partial / failed with a coarse failure kind, the reply within the
  cap and the whole reply behind a handle, an optional parsed JSON reply
  (`output_schema`, re-asked twice), the patch handle, spend, rounds, and
  the commands the lane's gate refused. Lane notes (a refusal, a nudge, a
  compaction, a retry) reach hosts; `x` stops the watched lane in the CLI,
  ✕ in the desktop.
- **Depth two, one wallet.** `max_depth` 2: root → orchestrating lanes →
  leaves. A lane below the cap gets its own fleet tools on a child spawner
  (spawns under the lane, stops with it); leaves get none. Every lane of a
  root turn spends from one `TreeBudget` (tokens / calls / spawns; spawns
  admitted against it, calls charged to it), and a lane whose turn hits it
  stops with what it has — a partial result, never an error the root can't
  use. Lane and leaf prompt appendices say what each may do.
- **Context as a variable.** Tool results over 30k chars are parked behind
  a `<<ccr:HASH>>` handle (head shown); `retrieve_original` reads a slice
  (`lines`), a grep, or splits into parked `chunks`; `spawn_agents.inputs`
  hands lanes handles, never content; `ask_model` answers up to 32
  tool-less prompts in parallel on the smol role over named inputs (the
  RLM `llm_query_batched`); siblings stagger 3 s so one request warms the
  prompt cache for the rest.
- **`map_agents`.** One lane (or one cheap call, `leaf`) per item from a
  template, coverage guaranteed by construction, one typed row per item,
  rows memoized by (item, task, schema) so a stopped run re-issued only
  runs what's left; `reduce: "agent"` folds the rows.
- **`fork: true`.** A lane that starts from a copy of the parent's
  conversation (the agent publishes a snapshot right before a forking
  spawn), for work that depends on what was read and decided.
- **The hub.** `fleet_started` / `fleet_finished` trajectory lines in the
  developer log; `/agents` (+ `read <n|id>`) in the CLI; a collapsed
  "finished agents" list under the desktop's fleet panel.

Deliberately not built: an IRC bus between lanes, role-split orchestrators,
a script sandbox before the declarative tool proves insufficient, and a
tree-wide semaphore (a lane holding a slot while waiting on its children
would deadlock — `max_spawns` bounds the tree instead).

**Robustness pass, same day.** Agent-tool results are never re-parked
(a 48k fleet document was being cut to a 4k head by the new tool cap);
`send_to_agent` refuses a lane that is still running; lanes no longer
write the system prompt per transcript and forks start from one snapshot;
the `map_agents` memo persists beside the session; the tree budget defers
its reset while a background fleet is in flight; `fleet.budget` events
show the tree's spend live in both hosts; Enter steers the watched lane in
the CLI; a finished lane opens in the desktop inspector from the hub.

**Background and navigation pass, same day.** The task registry
snapshots itself and bumps a change feed; the host forwards `tasks.changed`
and serves list/kill routes without the agent lock; the CLI has `/tasks`
(`kill <n>`), `/agents show <n>` (a lane's whole transcript), and a watch
pane a third of the terminal tall; `map_agents` runs in the background with
`wait: false`. The desktop follows a running lane live in the inspector,
steers the watched lane from a one-line box, and lists background commands
with a stop each.

**Field-failure pass, same day.** The first real run (a Qwen model
through the Oxen hub, asked to research NFL games) exposed three gaps.
A proxy that hit its upstream timeout mid `spawn_agents` ended the stream
*cleanly* — finish reason, `[DONE]`, no usage — with the call's arguments
stopped at `{"agents": `; the client took that as a finished reply and the
model burned two rounds apologising for "its" malformed call. A tool call
whose arguments end mid-value (serde's EOF class, not healable, not the
reply's own token limit) is now a transient stream failure: the request is
retried like a dropped connection and nothing of the fragment reaches the
transcript (`cut_off_call` in `call.rs`; `finish_reason` joins the
`requests.jsonl` line). A lane's auto-denied command now emits the
resolved-as-denied event, so hosts show "could not run … (needs
approval)" on the lane instead of a quiet lane. And the root model had no
guidance on *when* to delegate — only tool descriptions — so the system
prompt carries a delegation guideline when the agent tools are registered
(`OptionalTools.agents`; stripped for leaves), and the CLI's idle prompt
now keeps painting a background fleet's block, prints a one-line wrap-up
when it finishes (`agents finished: cowboys ✓ 72s · …`), steers a watched
lane on Enter, and notes commands still running in the background.

Still open: an offline TUI scenario for the lane block, and a second
real-model run now that the first one's failures are fixed.

## What's left / next

- [ ] Run-time GUI smoke test of the desktop app (`cargo tauri dev`), incl. live
      theme switching + vibe-code generation.
- [ ] Live end-to-end test against the real Oxen endpoint with a key, and a real
      `llama-server` run of a local model (this machine lacked the binary).
- [ ] Broaden `~/.oxen-harness/config.toml` beyond the active theme
      (host, defaults) — the selected model now persists via `models.json`.
- [ ] Switch local models mid-session (currently chosen at startup via `--local`;
      the desktop starts a fresh session on local switches).
- [ ] Per-theme palette swatches in the app theme list.

---

## Infrastructure TODOs (Cross-Phase)

- [x] CI workflow running the verification loop (fmt + clippy + tests) on push
      (`.github/workflows/ci.yml`, badge in the README).
- [x] Persist/restore previous sessions in the CLI (`--resume <id>` /
      `--continue`) and the desktop app (per-session agents).

## Recent — subagent hardening and agent hub (2026-09-13)

The review findings are fixed across isolation, lifecycle, tool preferences,
budgets, caching, and both front ends. Lane results are saved before completion;
committed and uncommitted edits survive cleanup and isolated follow-ups.
Tool-less leaves share cancellation and request admission with full agents.
Map resumption is explicit through `run_id`, with source/configuration checks
and `refresh`.

The CLI now has an `/agents` chooser, stable IDs, numbered live rows, previous/
next shortcuts, fleet switching, explicit message targets, and watch/send/stop/
follow-up/patch commands. The desktop keeps live and finished descendants in
one minimal panel with retained drafts, acknowledged stop/send controls,
scroll-follow control, and reviewed patch application.

Verification: 1,259 Rust tests passed (3 skipped); workspace fmt and Clippy passed;
466 frontend tests and TypeScript passed; Tauri bridge Clippy passed. Browser
inspection covered light/dark and a 390px viewport with no horizontal overflow.
The dedicated review/refactor pass is complete and documented in
`plans/subagent-hardening.md`.

## Composer responsiveness — 2026-09-18

Removed synchronous textarea layout reads and toolbar rerenders from typing.
Native content sizing has a frame-batched fallback for older webviews, including
rewrapping when the chat width changes. Chrome/WebKit checks and all 591 frontend
tests passed. Wider Rust check blockers and the review record are tracked in
`plans/composer-typing.md`.

## Chat scroll following — 2026-09-13

Fixed unexpected loss of automatic scrolling: streaming and layout changes follow
until the reader scrolls up; returning to the bottom, using the arrow, sending a
prompt, or switching chats resumes following. Twelve regression tests and real
browser checks cover the behavior. See `plans/chat-scroll.md` for verification
and existing workspace check blockers.

## Prairie arcade — 2026-09-18

All three existing cabinets now have richer pixel scenery and distinct mastery
mechanics: Dodge's earned Stampede, Hunt's focused shots and hit streaks, and
Trail's camp recovery/foraging/scouting with visible resource and river risks.
Cabinet controls sit outside the playfield, with explicit play/pause/menu buttons
and automatic pause when returning to the composer. River crossings no longer
skip the following landmark. See `plans/arcade-delight.md` for research,
verification, remaining workspace blockers, and the dedicated polish pass.

Arcade polish verified: 88 game tests, TypeScript, production build, and
Chrome/WebKit checks pass. The full frontend has 606 passing tests and two
unrelated sizing-baseline failures; current Rust checks and cache GC are
blocked by the existing API/lint failures documented in the feature plan.

## Recent — subagent budgets that scale (2026-09-19)

Diagnosed from a real run (session `18c059a7`): nineteen research agents spent
the 1.5M-token tree wallet in fifty seconds because the wallet charged the
gross prompt of every tool round, then every lane died at once with a notice
it could not act on, and the parent redid the work itself. Landed, one commit
each, in `crates/harness-agent`:

- the tree budget charges billable tokens (uncached prompt + cache writes +
  completion);
- a budget-stopped turn makes one tool-free final-report call, and a round
  cap is reported as `LaneStop::Rounds`;
- every lane opens with a wallet carved from its parent's remaining budget,
  folded back on close; admission refuses fleets the budget can't fund;
- lanes compress stale tool output and compact past 300k chars;
- lanes are warned at a fifth of their allowance, the subagent prompt names
  the allowance, and `spawn_agents` results carry a budget line;
- lane usage is recorded per lane, with `usage_for_tree` for the roll-up.
- 2026-09-22: a reply that "never arrived" after a fleet was a canvas call
  the model sent with no arguments; re-sending it drew a 400 the turn could
  not retry. Arguments that arrive as a JSON object (not a string) are now
  kept by the stream assembler, empty or cut-off arguments are re-sent as
  valid JSON, the tool result says the call arrived empty, and the desktop
  shows a refused canvas call as the error it was.
- 2026-09-25: an `open_file` (or canvas) the agent made landed in a right
  column the user had folded to its rail, so "they are looking at it now"
  was false. A tool-driven open for the chat on screen now expands the
  column the way a click on the rail does; a pinned work view says in the
  chat what it kept out of sight.
- follow-ups the same day: lane model calls are capped in flight across the
  whole tree (`max_tree_parallel`, default 4); tool-less leaves spend from
  their parent's remaining allowance instead of slicing it; a lane that
  failed on the provider is retried once from where it stopped; `map_agents`
  lanes are named for their items.

Verification: harness-agent 189 unit + 12 integration tests, harness-store 65,
workspace clippy and fmt clean.
