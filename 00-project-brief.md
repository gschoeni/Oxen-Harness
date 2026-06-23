# oxen-harness — Project Brief

**Purpose:** Condensed project context for all conversations. Vision, goals, current state, key decisions.
**Created:** 2026-06-21

---

## Vision

`oxen-harness` is an open source, hackable agentic coding harness — like Claude Code or Codex (OpenHands is a close comp) — written in Rust and powered by Oxen.ai. It runs an agent loop against any chat-completions model with tool calling, and records every turn so you can later fine-tune a model on your own coding traces.

## Goals

- A `claude`-style **interactive CLI** that edits code, runs commands, and drives git via tool calling.
- A cross-platform **Tauri v2 desktop app** (UI like Cursor agents / Claude Cowork), added once the core loop is stable.
- **Bring-your-own-model**: any Oxen.ai chat-completions model with tool calling; default `claude-opus-4-8`.
- **Exportable history**: full SQLite log of conversations + tool calls, with a JSONL exporter for fine-tuning.
- Stay small, readable, and forkable (Apache-2.0).

## Architecture / Approach

Single Cargo workspace of focused crates:

- `harness-core` — shared message/role types and defaults (leaf crate).
- `harness-llm` — Oxen.ai chat completions client (tool calling + SSE streaming); lightweight auth (env var or `auth_config.toml`).
- `harness-tools` — read (line-numbered)/write/edit files, glob find, regex search, sandboxed shell (with timeout), git status/diff/log/commit, Brave web search (when `BRAVE_API_KEY` is set), and `ask_user_question` for interviewing the user with multiple-choice questions when a decision is ambiguous.
- `harness-store` — SQLite history (verbatim) + JSONL export.
- `harness-local` — local models: curated Qwen3 GGUF catalog, managed downloads + disk tracking, `llama-server` launcher (OpenAI-compatible, no key).
- `harness-theme` — configurable, shareable themes (palette + voice) used by both front ends: built-ins (Oregon Trail/Midnight/Synthwave), TOML/JSON load+save with partial overrides, and the active-theme store under `~/.oxen-harness/`.
- `harness-agent` — the agent (Ralph) loop, wiring llm + tools + store together.
- `harness-cli` — the `oxen-harness` REPL binary (reusable interactive picker, `/theme` + `theme` subcommand, LLM-generated themes).

The agent loop: call model → if `finish_reason == tool_calls`, execute tools, append `tool` messages, repeat → stop on `stop`/`length`.

## Current Phase

**All phases implemented (0–6):** the Oxen client (streaming + tool calling), the
sandboxed tool set (fs/shell/git), the verbatim SQLite store, the agent loop, and
the interactive streaming CLI are built and tested (50 tests passing). The Tauri
v2 desktop app is scaffolded and its Rust bridge compiles. Remaining work is
run-time GUI verification, a live end-to-end test, and CI. See `02-status.md`.

## Key Decisions

- **Oxen.ai is the default provider** (OpenAI-compatible API); models are swappable, default `claude-opus-4-8`. **Local models** (Qwen3 via llama.cpp) are a first-class alternative through `--local <id>` / `models` subcommands / the desktop UI.
- **Lightweight auth** (no `liboxen` dependency): `OXEN_API_KEY` or parse `auth_config.toml` directly. (`liboxen` won't build without its heavy DuckDB/RocksDB tree — see `03-decisions.md`.)
- **SQLite history, stored verbatim**, with a JSONL exporter for fine-tuning.
- **SSE streaming in the REPL from day one**; single working directory per session; cross-platform from day one.
- **Themeable + shareable**: the CLI/app personality is data (palette + voice), persisted under `~/.oxen-harness/`, exportable/importable as a single TOML file, and creatable by "vibe" via the model. Default is Oregon Trail.
- Built with **The Ralph Wiggum loop** (tests-first, objective checks). Details in `03-decisions.md`.
