# Project Status & Roadmap

**Purpose:** Where we are, what's next, what's done. Pull this in for any working session.
**Updated:** 2026-07-14

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
| **7** | `harness-local`: local models via llama.cpp (Qwen3 GGUFs) | ✅ Complete |
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
> **649 Rust tests + 229 frontend tests passing**; CI runs fmt + clippy + tests + docs on the workspace, and tsc + vitest + bridge-clippy on the desktop app, on every push.

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

**Status:** ✅ Complete (13 tests passing)

- [x] `catalog`: curated Qwen3 GGUFs (`Q4_K_M`) from 0.6B → 32B + 30B-A3B MoE,
      with HF repo/file, approx size, and a hardware note (URLs HEAD-verified)
- [x] `ModelStore`: `~/.oxen-harness/models/` dir — installed status, per-model +
      total disk usage, streaming download (atomic `.part` → rename) with progress,
      and remove
- [x] `LocalServer`: locate `llama-server` (`LLAMA_SERVER` override or `PATH`), pick
      a free port, spawn against a GGUF (`--jinja` for tool calling), poll `/health`
      until loaded, and kill on drop (no leaked background server)
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
