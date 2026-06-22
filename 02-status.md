# Project Status & Roadmap

**Purpose:** Where we are, what's next, what's done. Pull this in for any working session.
**Updated:** 2026-06-21

---

## Phase Overview

| Phase | Goal | Status |
|-------|------|--------|
| **0** | Scaffold: workspace, crates, first green tests, KB, license | ✅ Complete |
| **2** | `harness-tools`: fs read/write/edit/search, sandboxed shell, git | ✅ Complete |
| **3** | `harness-store`: SQLite history (verbatim) + JSONL export | ✅ Complete |
| **1** | `harness-llm`: Oxen client — tool-calling types, auth, SSE streaming | ✅ Complete |
| **4** | `harness-agent`: the agent (Ralph) loop | ✅ Complete |
| **5** | `harness-cli`: interactive streaming REPL | ✅ Complete |
| **6** | `app/`: Tauri v2 cross-platform desktop app | ✅ Scaffolded (compiles) |

> Build order note: independent crates (tools, store) were built before the LLM
> client to keep each phase fast to verify. The agent loop lives in its own
> `harness-agent` crate (not `harness-core`) to avoid a dependency cycle.
> **82 tests passing** across the workspace.

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

**Status:** ✅ Complete (25 tests passing)

- [x] `Workspace` sandbox: path resolution rejecting escapes outside the root
- [x] `Tool` trait, `ToolRegistry` (dispatch by name), OpenAI tool definitions
- [x] fs tools: `read_file`, `write_file`, `edit_file` (unique-match), `search_files`
- [x] `run_shell`: command execution pinned to workspace root
- [x] `git`: status / diff / log / commit

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

**Status:** ✅ Complete (13 tests; binary verified)

- [x] `oxen-harness` binary with clap args (`--model`, `--workspace`, `--base-url`, `--host`)
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
- [x] Graceful, helpful exit when no API key is configured

---

## Phase 6 — Tauri v2 desktop app (`app/`)

**Status:** ✅ Scaffolded; Rust bridge compiles + clippy-clean

- [x] Separate Cargo project (excluded from the core workspace) so core stays fast
- [x] `src-tauri` bridge: `run_turn` + `session_info` commands over `harness-agent`
- [x] Live streaming to the UI via `agent://token` / `agent://tool` events
- [x] Dependency-free chat frontend (Cursor-agents-style) using `withGlobalTauri`
- [x] Tauri v2 capability granting `core:default`; valid app icon
- [ ] Run-time GUI verification (needs a desktop session + API key; `cargo tauri dev`)
- [ ] App icons for bundling + enable `bundle.active` for installers

---

## What's left / next

- [ ] Run-time GUI smoke test of the desktop app (`cargo tauri dev`).
- [ ] Live end-to-end test against the real Oxen endpoint with a key.
- [ ] CI workflow running the verification loop (fmt + clippy + tests) on push.
- [ ] `/model` validation + a config file (`~/.config/oxen-harness/config.toml`).

---

## Infrastructure TODOs (Cross-Phase)

- [ ] CI workflow running the verification loop (fmt + clippy + tests) on push.
- [x] Persist/restore previous sessions in the CLI (`--resume <id>`). App resume
      still pending.
