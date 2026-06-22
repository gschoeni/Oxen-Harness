# Working Decisions & Rationale

**Purpose:** Currently relevant decisions with enough "why" to be useful during implementation. For full deep-dive analysis, cite the source in each entry.
**Updated:** 2026-06-21

---

## Provider & Models

**Oxen.ai is the only provider** (2026-06-21)
The harness targets the Oxen.ai OpenAI-compatible chat completions API (`https://hub.oxen.ai/api/ai`, endpoint `/chat/completions`). Any model with tool calling works; the provider is fixed but the model is swappable. Default model is `claude-opus-4-8`.
-> *Full context: https://docs.oxen.ai/examples/inference/chat_completions*

## Auth & Config

**Lightweight auth; no `liboxen` dependency** (2026-06-21, revised)
Auth resolves from `OXEN_API_KEY`, falling back to parsing the Oxen
`auth_config.toml` directly (`$OXEN_CONFIG_DIR` or `~/.config/oxen/`, looked up
by host `hub.oxen.ai`). This is the same file the `oxen` CLI writes on login, so
`oxen login` interoperates — without taking the dependency.

*Why revised:* we initially chose a hard `liboxen` dependency, but empirically
`liboxen` does not compile with `default-features = false` (its source imports
`duckdb` unconditionally — 74 compile errors), so a real dependency forces the
full bundled **DuckDB + RocksDB C++ build**: a multi-minute first compile plus a
`cmake`/C++ toolchain prereq on every platform — all just to read an API token
from a TOML file. We dropped it in favor of a ~40-line parser with identical
behavior. `liboxen` can return later behind an optional feature for genuine data
versioning (see `04-backlog.md`).

**Configurable API host** (2026-06-21)
The base URL is resolvable, not hardcoded. Precedence: `--base-url` > `--host` >
`OXEN_BASE_URL` env > `OXEN_HOST` env > default `https://hub.oxen.ai/api/ai`. A
bare host (`localhost:3001`) expands to `http://<host>/api/ai` for local/loopback
hosts or explicit non-443 ports, `https://<host>/api/ai` otherwise; a value with a
scheme is used as-is. The auth token is looked up by the *resolved* host (e.g.
`localhost:3001`), matching how the `oxen` CLI keys tokens, so self-hosted/dev
servers work with `OXEN_API_KEY` or a per-host login. Helpers live in
`harness-llm::auth` (`resolve_base_url`, `base_url_from_host`, `host_from_base_url`).

## Tooling parity

**Essential tool set modeled on Claude Code (no MCP, no orchestration/network)** (2026-06-21)
After researching Claude Code's built-in tools (Read, Write, Edit, Bash, Glob,
Grep, plus Task/TodoWrite/WebFetch/WebSearch), we matched the *file + shell*
primitives a strong coding agent needs and deliberately stopped there:

- `read_file` returns `cat -n` line numbers with `offset`/`limit` and truncation
  caps (2000 lines / 2000 chars per line) — mirrors Claude's `Read` so models can
  cite/edit by line and read large files in chunks. Edit args must exclude the prefix.
- `find_files` = Claude's `Glob` (glob via `globset`, gitignore-aware, newest-first).
- `search_files` upgraded from literal substring to a `Grep`-style **regex** search
  (`regex` crate) with `content`/`files_with_matches`/`count` modes + `glob`/`path` filters.
- `run_shell` got a `timeout_ms` (default 120s) and a 30k-char output cap.

*Deliberately skipped* to keep the codebase simple: `TodoWrite`/`Task` (orchestration
+ session state), `WebFetch`/`WebSearch` (network + HTML parsing deps), `NotebookEdit`,
and anything MCP. New deps were limited to `regex` + `globset` (both already in the
`ignore` transitive tree). MCP remains a future opt-in (see `04-backlog.md`).

## History & Export

**SQLite, stored verbatim, JSONL export** (2026-06-21)
Conversation messages and tool inputs/outputs are persisted verbatim (no truncation caps) in SQLite, so traces are complete enough to fine-tune on. A JSONL exporter emits one message object per line for dataset building. Verbatim storage means large file/shell outputs are stored in full — acceptable for a local, single-user tool.

## Agent Loop

**Objective-check-driven (Ralph Wiggum) loop** (2026-06-21)
Development follows a tight test-first loop: read spec → write/adjust a test → smallest change → write to disk → run `fmt`/`clippy`/tests → fix root cause → stop on green. The *runtime* agent loop mirrors this: call model → execute any `tool_calls` → append `tool` messages → repeat until `finish_reason` is `stop`/`length`.
-> *Full context: `AGENTS.md`*

## UX & Scope

**CLI first, Tauri later; stream from day one** (2026-06-21)
Ship a `claude`-style interactive REPL with live SSE token streaming first; add the cross-platform Tauri v2 desktop app once the core loop is stable. Sessions are scoped to a single working directory. Shell commands run in a sandboxed working directory where possible, but the model decides. File edits are allowed by default. Cross-platform support is a constraint from day one.

**Oregon-Trail-themed CLI, hand-rolled styling** (2026-06-21)
The REPL's structure follows modern coding CLIs (welcome panel, in-place status
spinner, transparent tool lines) and its voice is the 1980s Oregon Trail game —
a natural fit since *oxen* pull the trail's wagons and Oxen.ai powers this one.
All flavor + rendering lives in `harness-cli/src/theme.rs`. Styling is hand-rolled
(24-bit ANSI, a tiny block-letter "figlet", a time-seeded xorshift for phrase
picks, a background-thread spinner) rather than pulling `owo-colors`/`ratatui`/
`indicatif` — it keeps deps minimal and the build fast, and the surface is small.
Color auto-disables for non-TTY / `NO_COLOR` / `TERM=dumb` so piped output is clean.

## Tooling

**Single Cargo workspace, focused crates** (2026-06-21)
`harness-core` (base types) / `harness-llm` / `harness-tools` / `harness-store` /
`harness-agent` (orchestration loop) / `harness-cli`. Tests use `mockito` to fake
the HTTP endpoint (deterministic, offline). `cargo nextest` is the preferred test
runner.

**Orchestration lives in `harness-agent`, not `harness-core`** (2026-06-21)
The loop depends on the llm/tools/store crates, which all depend on `harness-core`
for shared types. Putting the loop in `core` would create a dependency cycle, so a
dedicated `harness-agent` crate sits above them. `harness-core` stays a leaf of
shared domain types.

**`HistoryStore` is `Send + Sync`** (2026-06-21)
The SQLite connection is wrapped in a `Mutex` so the store can be shared via `Arc`
across threads (the agent loop today, the Tauri app later).
