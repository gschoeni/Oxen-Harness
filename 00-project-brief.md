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
- `harness-tools` — read/write/edit/search files, sandboxed shell, git status/diff/log/commit.
- `harness-store` — SQLite history (verbatim) + JSONL export.
- `harness-agent` — the agent (Ralph) loop, wiring llm + tools + store together.
- `harness-cli` — the `oxen-harness` REPL binary.

The agent loop: call model → if `finish_reason == tool_calls`, execute tools, append `tool` messages, repeat → stop on `stop`/`length`.

## Current Phase

**Core loop working (Phases 0–4 complete):** the Oxen client (streaming + tool
calling), the tool set (fs/shell/git, sandboxed), the verbatim SQLite store, and
the agent loop are all built and tested (43 tests passing). Next up: Phase 5 —
the interactive streaming CLI REPL. See `02-status.md`.

## Key Decisions

- **Oxen.ai is the only provider** (OpenAI-compatible API); models are swappable, default `claude-opus-4-8`.
- **Lightweight auth** (no `liboxen` dependency): `OXEN_API_KEY` or parse `auth_config.toml` directly. (`liboxen` won't build without its heavy DuckDB/RocksDB tree — see `03-decisions.md`.)
- **SQLite history, stored verbatim**, with a JSONL exporter for fine-tuning.
- **SSE streaming in the REPL from day one**; single working directory per session; cross-platform from day one.
- Built with **The Ralph Wiggum loop** (tests-first, objective checks). Details in `03-decisions.md`.
